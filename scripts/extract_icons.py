"""ゲームクライアントから属性・モジュールのアイコンを抽出し、縮小して PNG で書き出す。

使い方:
    python -I scripts/extract_icons.py <container ディレクトリ> [出力ディレクトリ]

    <container> は <ゲーム>/StarASIA_STEAM_Data/StreamingAssets/container（m*.pkg の置き場）。
    出力ディレクトリの既定は src/assets/icons。container は読み取りだけで、何も書き込まない。

前提:
    - UnityPy と Pillow（pip install UnityPy）。動作確認は UnityPy 1.25 / Pillow 12.2。
    - pkg は暗号化されていない UnityFS バンドルを連結したもの。どの pkg に入っているかは
      ゲームの更新で変わるため、番号を決め打ちせず全 pkg を走査してアセット名で探す
      （圧縮されたバンドル内の名前は pkg の生バイト列からは見えないので、事前の絞り込みはしない。
      全 pkg の走査は CPU 並列で行う）。

出力（ID ベースのファイル名）:
    attr_<attr_id>.png      属性アイコン（Sprite・元 80x80）を 32px に縮小
    module_<config_id>.png  モジュールアイコン（Texture2D・元 256x256）を 48px に縮小
    縮小は LANCZOS。表示サイズ（属性 16px・モジュール 20px）の 2 倍を目安にしている。

出典（アセット名と ID の対応）:
    - 属性: ゲームの m0 のデータ表と BPSR-ZDPS（MIT）の Data/ModEffectTable.json の Icon 列。
    - モジュール: config_id 末尾 01/02/03/04 が段階 2/3/4/5。BPSR-ZDPS の ModuleSolver.cs
      （1268-1286 行付近）と同じ対応。
    アイコン画像の権利はゲームの権利者に帰属する。
"""

import mmap
import os
import re
import struct
import sys
from concurrent.futures import ProcessPoolExecutor
from pathlib import Path

import UnityPy
from PIL import Image

MAGIC = b"UnityFS\x00"
ATTR_SIZE = 32
MODULE_SIZE = 48

# attr_id -> Sprite 名の番号部（mod_effect_icon_<番号>）
ATTR_ICONS: dict[int, str] = {
    1110: "011", 1111: "039", 1112: "034", 1113: "002", 1114: "038",
    1205: "023", 1206: "042", 1307: "009", 1308: "014",
    1407: "013", 1408: "026", 1409: "019", 1410: "048",
    2104: "016", 2105: "006", 2204: "012", 2205: "024",
    2304: "045", 2404: "004", 2405: "035", 2406: "017",
}  # fmt: skip

# 種別（攻撃 55001xx / 支援 55002xx / 防御 55003xx）ごとの、段階 2〜5 の Texture2D 名。
MODULE_ICONS: dict[int, list[str]] = {
    5500100: ["item_mod_device_attack2", "item_mod_device_attack3",
              "item_mod_device_attack4", "item_icons_mod_device_attack5"],
    5500200: ["item_mod_device_2", "item_mod_device_3",
              "item_mod_device_4", "item_icons_mod_device_5"],
    5500300: ["item_mod_device_protect2", "item_mod_device_protect3",
              "item_mod_device_protect4", "item_icons_device_protect5"],
}  # fmt: skip


def build_targets() -> dict[tuple[str, str], str]:
    """(Unity 型名, アセット名) -> 出力ファイル名。"""
    targets: dict[tuple[str, str], str] = {}
    for attr_id, num in ATTR_ICONS.items():
        targets[("Sprite", f"mod_effect_icon_{num}")] = f"attr_{attr_id}.png"
    for base, names in MODULE_ICONS.items():
        for i, name in enumerate(names, start=1):
            targets[("Texture2D", name)] = f"module_{base + i}.png"
    return targets


def bundle_size(buf: mmap.mmap, off: int) -> int:
    """UnityFS ヘッダ（signature, version, 2 つの文字列の次の int64）からバンドル全長を読む。"""
    p = off + len(MAGIC) + 4
    p = buf.find(b"\x00", p) + 1
    p = buf.find(b"\x00", p) + 1
    return struct.unpack(">q", buf[p : p + 8])[0]


def scan_pkg(path: str) -> list[tuple[str, str, str, tuple[int, int], bytes]]:
    """1 つの pkg から対象アセットを探す。(型, 名前, モード, サイズ, 画素) のリストを返す。"""
    targets = build_targets()
    found = []
    with open(path, "rb") as f, mmap.mmap(f.fileno(), 0, access=mmap.ACCESS_READ) as mm:
        for m in re.finditer(re.escape(MAGIC), mm):
            off = m.start()
            try:
                env = UnityPy.load(mm[off : off + bundle_size(mm, off)])
            except Exception as e:  # 壊れたバンドルは飛ばすが、黙らず知らせる
                print(f"warn: {os.path.basename(path)}@{off}: {type(e).__name__}: {e}", file=sys.stderr)
                continue
            for obj in env.objects:
                tname = obj.type.name
                if tname not in ("Sprite", "Texture2D"):
                    continue
                try:
                    name = obj.peek_name() or ""
                except Exception:
                    continue
                if (tname, name) not in targets:
                    continue
                img = obj.read().image.convert("RGBA")
                found.append((tname, name, img.mode, img.size, img.tobytes()))
    return found


def main() -> int:
    if len(sys.argv) not in (2, 3):
        print(__doc__, file=sys.stderr)
        return 2
    container = Path(sys.argv[1])
    out_dir = Path(sys.argv[2]) if len(sys.argv) == 3 else Path(__file__).resolve().parent.parent / "src" / "assets" / "icons"
    pkgs = sorted(container.glob("m*.pkg"))
    if not pkgs:
        print(f"error: {container} に m*.pkg がありません", file=sys.stderr)
        return 1

    targets = build_targets()
    # 同じ (型, 名前) が複数の pkg にあれば最初のものを使う。画素が食い違う場合は警告する。
    hits: dict[tuple[str, str], tuple[str, tuple[int, int], bytes, str]] = {}
    with ProcessPoolExecutor() as ex:
        for pkg, found in zip(pkgs, ex.map(scan_pkg, map(str, pkgs))):
            for tname, name, mode, size, pixels in found:
                key = (tname, name)
                if key in hits:
                    if hits[key][2] != pixels:
                        print(f"warn: {name} が複数の pkg にあり内容が異なる（{hits[key][3]} を採用、{pkg.name} は無視）", file=sys.stderr)
                    continue
                hits[key] = (mode, size, pixels, pkg.name)

    missing = sorted(name for (tname, name) in targets if (tname, name) not in hits)
    if missing:
        print("error: 次のアセットが見つかりません:", file=sys.stderr)
        for name in missing:
            print(f"  {name}", file=sys.stderr)
        return 1

    out_dir.mkdir(parents=True, exist_ok=True)
    for key, filename in sorted(targets.items(), key=lambda kv: kv[1]):
        mode, size, pixels, pkg_name = hits[key]
        if size[0] != size[1]:
            print(f"error: {key[1]} が正方形ではありません: {size}", file=sys.stderr)
            return 1
        img = Image.frombytes(mode, size, pixels)
        side = ATTR_SIZE if filename.startswith("attr_") else MODULE_SIZE
        img = img.resize((side, side), Image.Resampling.LANCZOS)
        img.save(out_dir / filename, optimize=True)
        print(f"{filename}: {key[1]} ({key[0]}, {pkg_name}, {size[0]}x{size[1]} -> {side}px)")
    print(f"{len(targets)} 枚を {out_dir} に書き出しました")
    return 0


if __name__ == "__main__":
    sys.exit(main())
