import { X } from "lucide-react";
import type { AttrMeta, AttrState } from "../types";
import { CATEGORY_LABELS } from "../types";

interface Props {
  attributes: AttrMeta[];
  selection: Record<number, AttrState>;
  requireLevels: Record<number, number>;
  category: string;
  onRemoveAttr: (id: number) => void; // 目標/除外を解除（none に）
  onResetCategory: () => void; // カテゴリを「すべて」に戻す
}

/** 適用中の検索条件をチップ表示し、×で個別に解除できるサマリーバー。 */
export function ConditionSummary({
  attributes,
  selection,
  requireLevels,
  category,
  onRemoveAttr,
  onResetCategory,
}: Props) {
  const nameOf = new Map(attributes.map((a) => [a.id, a.name]));
  const targets: number[] = [];
  const excludes: number[] = [];
  for (const [id, s] of Object.entries(selection)) {
    if (s === "target") targets.push(Number(id));
    else if (s === "exclude") excludes.push(Number(id));
  }

  const hasCategory = category !== "all";
  if (targets.length === 0 && excludes.length === 0 && !hasCategory) return null;

  return (
    <div className="mb-2 flex flex-wrap items-center gap-1.5">
      <span className="text-[11px] text-slate-500">条件</span>

      {targets.map((id) => {
        const lv = requireLevels[id] ?? 0;
        return (
          <span
            key={`t${id}`}
            className="flex items-center gap-1 rounded-full border border-emerald-500/50 bg-emerald-500/10 py-0.5 pl-2 pr-1 text-[11px] text-emerald-200"
          >
            {nameOf.get(id) ?? `#${id}`}
            {lv > 0 && <span className="text-emerald-400/80">Lv≥{lv}</span>}
            <button
              onClick={() => onRemoveAttr(id)}
              aria-label={`目標「${nameOf.get(id) ?? id}」を解除`}
              title="解除"
              className="rounded-full p-0.5 transition hover:bg-emerald-500/20"
            >
              <X size={11} />
            </button>
          </span>
        );
      })}

      {excludes.map((id) => (
        <span
          key={`e${id}`}
          className="flex items-center gap-1 rounded-full border border-rose-500/50 bg-rose-500/10 py-0.5 pl-2 pr-1 text-[11px] text-rose-300"
        >
          <span className="line-through">{nameOf.get(id) ?? `#${id}`}</span>
          <button
            onClick={() => onRemoveAttr(id)}
            aria-label={`除外「${nameOf.get(id) ?? id}」を解除`}
            title="解除"
            className="rounded-full p-0.5 transition hover:bg-rose-500/20"
          >
            <X size={11} />
          </button>
        </span>
      ))}

      {hasCategory && (
        <span className="flex items-center gap-1 rounded-full border border-indigo-500/50 bg-indigo-500/10 py-0.5 pl-2 pr-1 text-[11px] text-indigo-200">
          {CATEGORY_LABELS[category] ?? category}
          <button
            onClick={onResetCategory}
            aria-label="カテゴリ条件を解除"
            title="解除"
            className="rounded-full p-0.5 transition hover:bg-indigo-500/20"
          >
            <X size={11} />
          </button>
        </span>
      )}
    </div>
  );
}
