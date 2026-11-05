import { attrIconUrl, moduleIconUrl } from "../icons";

// アイコンは属性選択欄（AttributePicker）と結果カードのモジュール一覧（SolutionCard）だけに付ける。
// 内訳・条件サマリー・比較表など他の場所には付けない（表示の密度と一貫性を保つため）。

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

// className は状態に連動した見た目（除外時の灰色化など）を呼び出し側から渡すためのもの。
export function AttrIcon({
  attrId,
  size,
  className,
}: {
  attrId: number;
  size: number;
  className?: string;
}) {
  return <Icon src={attrIconUrl(attrId)} size={size} className={className} />;
}

// 20px のアイコンは見出しの行高（16px）を超えるので、上下の負マージンで行高を増やさない。
export function ModuleIcon({ configId, size }: { configId: number; size: number }) {
  return <Icon src={moduleIconUrl(configId)} size={size} className="-my-0.5" />;
}
