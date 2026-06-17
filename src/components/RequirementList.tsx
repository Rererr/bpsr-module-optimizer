import type { AttrMeta } from "../types";

interface Props {
  targets: AttrMeta[];
  /// attr_id -> 下限レベル（0=制約なし）
  levels: Record<number, number>;
  onChange: (id: number, level: number) => void;
}

const LEVELS = [0, 1, 2, 3, 4, 5, 6];

export function RequirementList({ targets, levels, onChange }: Props) {
  if (targets.length === 0) {
    return (
      <p className="rounded-lg bg-slate-800/40 px-3 py-2.5 text-[11px] leading-relaxed text-slate-500">
        目標属性（緑）を選ぶと、ここで属性ごとに下限Lvを設定できます。
      </p>
    );
  }

  return (
    <div className="flex flex-col gap-1.5">
      {targets.map((a) => {
        const lv = levels[a.id] ?? 0;
        return (
          <div
            key={a.id}
            className="flex items-center justify-between gap-2 rounded-lg bg-slate-800/50 px-2.5 py-1.5"
          >
            <span className="flex min-w-0 items-center gap-1.5">
              <span className="h-2 w-2 shrink-0 rounded-full bg-emerald-500" />
              <span className="truncate text-xs text-slate-200" title={a.name}>
                {a.name}
              </span>
            </span>
            <select
              value={lv}
              onChange={(e) => onChange(a.id, Number(e.currentTarget.value))}
              className={`shrink-0 rounded-md border px-1.5 py-1 text-[11px] outline-none transition ${
                lv > 0
                  ? "border-indigo-500/60 bg-indigo-500/15 text-indigo-200"
                  : "border-slate-700 bg-slate-900 text-slate-400"
              }`}
            >
              {LEVELS.map((n) => (
                <option key={n} value={n} className="bg-slate-900 text-slate-200">
                  {n === 0 ? "下限なし" : `Lv${n}以上`}
                </option>
              ))}
            </select>
          </div>
        );
      })}
    </div>
  );
}
