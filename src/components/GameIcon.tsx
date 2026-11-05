import { attrIconUrl, moduleIconUrl } from "../icons";

// 装飾アイコン。横に名前の文字があるので alt は空にする。未知の ID は何も描画しない。
function Icon({ src, size, className }: { src: string | undefined; size: number; className?: string }) {
  if (!src) return null;
  return (
    <img
      src={src}
      alt=""
      width={size}
      height={size}
      draggable={false}
      className={`shrink-0 ${className ?? ""}`}
    />
  );
}

export function AttrIcon({ attrId, size }: { attrId: number; size: number }) {
  return <Icon src={attrIconUrl(attrId)} size={size} />;
}

// 20px のアイコンは見出しの行高（16px）を超えるので、上下の負マージンで行高を増やさない。
export function ModuleIcon({ configId, size }: { configId: number; size: number }) {
  return <Icon src={moduleIconUrl(configId)} size={size} className="-my-0.5" />;
}
