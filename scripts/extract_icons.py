"""ゲームクライアントから属性・モジュールのアイコンを抽出し、縮小して PNG で書き出す。

使い方:
    python -I scripts/extract_icons.py <container ディレクトリ> [出力ディレクトリ]

    <container> は <ゲーム>/StarASIA_STEAM_Data/StreamingAssets/container（m*.pkg の置き場）。
    出力ディレクトリの既定は src/assets/icons。container は読み取りだけで、何も書き込まない。

前提:
    - UnityPy と Pillow（pip install UnityPy）。動作確認は UnityPy 1.25 / Pillow 12.2。
    - pkg は暗号化されていない UnityFS バンドルを連結したもの。どの pkg に入っているかは
      ゲームの更新で変わるため、番号を決め打ちせず container 配下（Patch/ など下位も含む）の
      全 pkg を走査してアセット名で探す。同じ名前で中身の違うアセットが複数あれば、
      どれを採るか決められないのでエラーで止める
      （圧縮されたバンドル内の名前は pkg の生バイト列からは見えないので、事前の絞り込みはしない。
      全 pkg の走査は CPU 並列で行う）。

出力（ID ベースのファイル名）:
    attr_<attr_id>.png      属性アイコン（Sprite・元 80x80）を 32px に縮小
    module_<config_id>.png  モジュールアイコン（Texture2D・元 256x256）を 48px に縮小
    縮小は LANCZOS。表示サイズ（属性 16px・モジュール 20px）の 2 倍を目安にしている。

出典（アセット名と ID の対応）:
    - 属性: ゲームの m0 のデータ表と BPSR-ZDPS（MIT）の Data/ModEffectTable.json の
      EffectConfigIcon 列。
    - モジュール: config_id 末尾 01/02/03/04 が段階 2/3/4/5。BPSR-ZDPS の
      BPSR-ZDPS/Windows/ModuleSolver.cs:1270-1283 と同じ対応。
    アイコン画像の権利はゲームの権利者に帰属する。
"""

import mmap
import os
import re
import struct
import sys
from concurrent.futures import ProcessPoolExecutor
from pathlib import Path
from typing import NamedTuple

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


class Target(NamedTuple):
    filename: str  # 出力ファイル名
    side: int  # 縮小後の一辺（px）


def build_targets() -> dict[tuple[str, str], Target]:
    """(Unity 型名, アセット名) -> 出力ファイル名と縮小サイズ。"""
    targets: dict[tuple[str, str], Target] = {}
    for attr_id, num in ATTR_ICONS.items():
        targets[("Sprite", f"mod_effect_icon_{num}")] = Target(f"attr_{attr_id}.png", ATTR_SIZE)
    for base, names in MODULE_ICONS.items():
        for i, name in enumerate(names, start=1):
            targets[("Texture2D", name)] = Target(f"module_{base + i}.png", MODULE_SIZE)
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
                except Exception as e:  # 名前を読めないオブジェクトは飛ばすが、黙らず知らせる
                    print(f"warn: {os.path.basename(path)}@{off}: peek_name 失敗: {type(e).__name__}: {e}", file=sys.stderr)
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
    # Patch/ などの下位ディレクトリに更新分が入っても拾う。
    pkgs = sorted(container.rglob("m*.pkg"))
    if not pkgs:
        print(f"error: {container} に m*.pkg がありません", file=sys.stderr)
        return 1

    targets = build_targets()
    hits: dict[tuple[str, str], tuple[str, tuple[int, int], bytes, str]] = {}
    conflicts: list[str] = []
    with ProcessPoolExecutor() as ex:
        for pkg, found in zip(pkgs, ex.map(scan_pkg, map(str, pkgs))):
            where = str(pkg.relative_to(container))
            for tname, name, mode, size, pixels in found:
                key = (tname, name)
                if key not in hits:
                    hits[key] = (mode, size, pixels, where)
                elif hits[key][:3] != (mode, size, pixels):
                    conflicts.append(f"{name}: {hits[key][3]} と {where} で内容が異なる")

    # 書き出しの前に全件を検査する（途中まで書いて失敗する経路を作らない）。
    errors: list[str] = []
    if conflicts:
        errors.append("同じ名前で中身の違うアセットが複数あり、どれを使うか決められません:")
        errors.extend(f"  {c}" for c in conflicts)
    missing = sorted(name for (tname, name) in targets if (tname, name) not in hits)
    if missing:
        errors.append("次のアセットが見つかりません:")
        errors.extend(f"  {name}" for name in missing)
    images: dict[str, Image.Image] = {}
    for key, target in sorted(targets.items(), key=lambda kv: kv[1].filename):
        if key not in hits:
            continue
        mode, size, pixels, _ = hits[key]
        if size[0] != size[1]:
            errors.append(f"{key[1]} が正方形ではありません: {size}")
            continue
        images[target.filename] = Image.frombytes(mode, size, pixels).resize(
            (target.side, target.side), Image.Resampling.LANCZOS
        )
    if errors:
        for line in errors:
            print(line if line.startswith("  ") else f"error: {line}", file=sys.stderr)
        return 1

    out_dir.mkdir(parents=True, exist_ok=True)
    for key, target in sorted(targets.items(), key=lambda kv: kv[1].filename):
        images[target.filename].save(out_dir / target.filename, optimize=True)
        _, size, _, where = hits[key]
        print(f"{target.filename}: {key[1]} ({key[0]}, {where}, {size[0]}x{size[1]} -> {target.side}px)")
    print(f"{len(targets)} 枚を {out_dir} に書き出しました")
    return 0


if __name__ == "__main__":
    sys.exit(main())
