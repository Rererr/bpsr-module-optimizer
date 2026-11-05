// ゲームから抽出したアイコン（scripts/extract_icons.py の出力）の引き当て。
// ファイル名は attr_<attr_id>.png / module_<config_id>.png。ID から URL を引くのはここだけ。

const urls = import.meta.glob<string>("./assets/icons/*.png", {
  eager: true,
  query: "?url",
  import: "default",
});

function iconUrl(kind: "attr" | "module", id: number): string | undefined {
  return urls[`./assets/icons/${kind}_${id}.png`];
}

/** 属性アイコンの URL。未知の属性IDは undefined（UI ではアイコンを出さない）。 */
export function attrIconUrl(attrId: number): string | undefined {
  return iconUrl("attr", attrId);
}

/** モジュールアイコンの URL。未知の config_id は undefined。 */
export function moduleIconUrl(configId: number): string | undefined {
  return iconUrl("module", configId);
}
