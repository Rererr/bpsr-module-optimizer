import type { AttrMeta, AttrState } from "../types";

export type { AttrState };

interface Props {
  attributes: AttrMeta[];
  selection: Record<number, AttrState>;
  onCycle: (id: number) => void;
  onClear: () => void;
}

const GROUP_ORDER = ["Basic", "Combat", "Focus", "Resist", "Support", "Ultra"];
const GROUP_LABELS: Record<string, string> = {
  Basic: "基礎ステータス",
  Combat: "攻撃",
  Focus: "集中",
  Resist: "耐性",
  Support: "回復/支援",
  Ultra: "上位(Ultra)",
};

function chipClass(state: AttrState, special: boolean): string {
  if (state === "target")
    return "border-emerald-500 bg-emerald-500/15 text-emerald-200 ring-1 ring-emerald-500/40";
  if (state === "exclude")
    return "border-rose-500 bg-rose-500/10 text-rose-300 line-through ring-1 ring-rose-500/30";
  return special
    ? "border-amber-700/50 bg-amber-500/5 text-amber-200/80 hover:border-amber-500"
    : "border-slate-700 bg-slate-800/60 text-slate-300 hover:border-slate-500";
}

export function AttributePicker({ attributes, selection, onCycle, onClear }: Props) {
  const byGroup = new Map<string, AttrMeta[]>();
  for (const a of attributes) {
    if (!byGroup.has(a.group)) byGroup.set(a.group, []);
    byGroup.get(a.group)!.push(a);
  }
  const groups = GROUP_ORDER.filter((g) => byGroup.has(g));

  const targetCount = Object.values(selection).filter((s) => s === "target").length;
  const excludeCount = Object.values(selection).filter((s) => s === "exclude").length;

  return (
    <div className="flex flex-col gap-3">
      <div className="flex items-center justify-between">
        <div className="flex items-center gap-3 text-xs">
          <span className="flex items-center gap-1.5 text-emerald-300">
            <span className="h-2 w-2 rounded-full bg-emerald-500" />
            目標 {targetCount}
          </span>
          <span className="flex items-center gap-1.5 text-rose-300">
            <span className="h-2 w-2 rounded-full bg-rose-500" />
            除外 {excludeCount}
          </span>
        </div>
        <button
          onClick={onClear}
          className="text-xs text-slate-400 underline-offset-2 hover:text-slate-200 hover:underline"
        >
          クリア
        </button>
      </div>

      <p className="text-[11px] leading-relaxed text-slate-500">
        クリックで <span className="text-emerald-300">目標</span> →{" "}
        <span className="text-rose-300">除外</span> → 解除 と切替。
        <span className="text-emerald-300">目標</span>はLv6到達を優先、
        <span className="text-rose-300">除外</span>はその属性を含む組み合わせを除外します。
      </p>

      <div className="flex flex-col gap-3">
        {groups.map((g) => (
          <div key={g}>
            <div className="mb-1.5 text-[11px] font-semibold uppercase tracking-wider text-slate-500">
              {GROUP_LABELS[g] ?? g}
            </div>
            <div className="flex flex-wrap gap-1.5">
              {byGroup.get(g)!.map((a) => {
                const st = selection[a.id] ?? "none";
                return (
                  <button
                    key={a.id}
                    onClick={() => onCycle(a.id)}
                    className={`rounded-full border px-2.5 py-1 text-[11px] font-medium transition ${chipClass(
                      st,
                      a.special,
                    )}`}
                  >
                    {a.name}
                  </button>
                );
              })}
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}
