import type { AttrMeta, AttrState } from "../types";
import { useI18n } from "../i18n";

export type { AttrState };

interface Props {
  attributes: AttrMeta[];
  selection: Record<number, AttrState>;
  onCycle: (id: number) => void;
  onClear: () => void;
  // true=ハード除外（該当モジュールを候補から丸ごと除外）、false=ソフト除外（既定。属性のみ
  // ランキング集計から除外し、モジュール自体は候補に残す）。
  hardExclude: boolean;
  onHardExcludeChange: (v: boolean) => void;
}

const GROUP_ORDER = ["Basic", "Combat", "Focus", "Resist", "Support", "Ultra"];

function chipClass(state: AttrState, special: boolean): string {
  if (state === "target")
    return "border-emerald-500 bg-emerald-500/15 text-emerald-200 ring-1 ring-emerald-500/40";
  if (state === "exclude")
    return "border-rose-500 bg-rose-500/10 text-rose-300 line-through ring-1 ring-rose-500/30";
  return special
    ? "border-amber-700/50 bg-amber-500/5 text-amber-200/80 hover:border-amber-500"
    : "border-slate-700 bg-slate-800/60 text-slate-300 hover:border-slate-500";
}

export function AttributePicker({
  attributes,
  selection,
  onCycle,
  onClear,
  hardExclude,
  onHardExcludeChange,
}: Props) {
  const { t, lang, attrName, groupLabel } = useI18n();
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
            {t("picker.targetN", { n: targetCount })}
          </span>
          <span className="flex items-center gap-1.5 text-rose-300">
            <span className="h-2 w-2 rounded-full bg-rose-500" />
            {t("picker.excludeN", { n: excludeCount })}
          </span>
        </div>
        <button
          onClick={onClear}
          className="text-xs text-slate-400 underline-offset-2 hover:text-slate-200 hover:underline"
        >
          {t("picker.clear")}
        </button>
      </div>

      <p className="text-[11px] leading-relaxed text-slate-500">
        {lang === "ja" ? (
          <>
            クリックで <span className="text-emerald-300">目標</span> →{" "}
            <span className="text-rose-300">除外</span> → 解除 と切替。
            <span className="text-emerald-300">目標</span>はLv6到達を優先、
            <span className="text-rose-300">除外</span>は既定でソフト除外
            （その属性はスコア評価から除外＝Lv6/Lv5数やレベル合計に数えません。同点になった
            場合のみ、巻き添えの少ない方を優先します）。下の切替でハード除外
            （その属性を含むモジュールを一切使わない）に変更できます。
          </>
        ) : (
          <>
            Click to cycle <span className="text-emerald-300">Target</span> →{" "}
            <span className="text-rose-300">Exclude</span> → off.{" "}
            <span className="text-emerald-300">Target</span> prioritizes reaching Lv6;{" "}
            <span className="text-rose-300">Exclude</span> defaults to a soft exclude (that
            attribute is dropped from scoring — not counted toward Lv6/Lv5 counts or the
            level total. Only when tied does it prefer the option with less of that
            attribute as a side effect). Use the toggle below for a hard exclude (never use
            modules containing that attribute).
          </>
        )}
      </p>

      <label className="flex items-center gap-2 text-[11px] text-slate-400">
        <input
          type="checkbox"
          checked={hardExclude}
          onChange={(e) => onHardExcludeChange(e.target.checked)}
          className="h-3.5 w-3.5 rounded border-slate-600 bg-slate-800 accent-rose-500"
        />
        {t("picker.hardExclude")}
      </label>

      <div className="flex flex-col gap-3">
        {groups.map((g) => (
          <div key={g}>
            <div className="mb-1.5 text-[11px] font-semibold uppercase tracking-wider text-slate-500">
              {groupLabel(g)}
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
                    {attrName(a.id, a.name)}
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
