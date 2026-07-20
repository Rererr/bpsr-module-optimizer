import { Trophy, Star, TriangleAlert } from "lucide-react";
import type { AttrMeta, Solution } from "../types";
import { useI18n } from "../i18n";

interface Props {
  solution: Solution;
  targetIds: Set<number>;
  // 選択中の目標属性メタ（結果タブのみ渡す）。含められなかった目標名の表示に使う。
  targetAttrs?: AttrMeta[];
  rank?: number; // 省略時はランクバッジ非表示（お気に入り表示で使用）
  isFavorite?: boolean;
  onToggleFavorite?: () => void; // 指定時のみ★トグルを表示
}

function qualityColor(q: number): string {
  switch (q) {
    case 4:
      return "text-purple-300 border-purple-500/40 bg-purple-500/10";
    case 3:
      return "text-sky-300 border-sky-500/40 bg-sky-500/10";
    case 2:
      return "text-emerald-300 border-emerald-500/40 bg-emerald-500/10";
    default:
      return "text-slate-300 border-slate-600 bg-slate-700/30";
  }
}

function rankColor(rank: number): string {
  if (rank === 1) return "bg-amber-400/20 text-amber-300 border-amber-400/40";
  if (rank === 2) return "bg-slate-300/15 text-slate-200 border-slate-300/30";
  if (rank === 3) return "bg-orange-500/15 text-orange-300 border-orange-500/30";
  return "bg-slate-700/40 text-slate-400 border-slate-600/40";
}

// 6段階レベルのドット表示（Lv6=金, Lv5=空色, それ以下=緑）。
function LevelDots({ level }: { level: number }) {
  const color = level === 6 ? "bg-amber-400" : level === 5 ? "bg-sky-400" : "bg-emerald-500";
  return (
    <span className="inline-flex gap-0.5">
      {Array.from({ length: 6 }).map((_, i) => (
        <span
          key={i}
          className={`h-1.5 w-1.5 rounded-full ${i < level ? color : "bg-slate-700"}`}
        />
      ))}
    </span>
  );
}

function levelTextColor(level: number, selected: boolean): string {
  if (level === 6) return "text-amber-300 font-semibold";
  if (level === 5) return "text-sky-300";
  return selected ? "text-emerald-300" : "text-slate-300";
}

export function SolutionCard({
  solution,
  targetIds,
  targetAttrs,
  rank,
  isFavorite,
  onToggleFavorite,
}: Props) {
  const { t, attrName, moduleName, categoryLabel } = useI18n();

  // 目標属性の包含状況。selected_present（Lv1以上で存在する目標数）が選択目標数に満たない場合、
  // 一部の目標が「含められず黙って除外」された合図なので、その時だけ警告チップで可視化する。
  const targetTotal = targetIds.size;
  const someTargetsMissing = targetTotal > 0 && solution.selected_present < targetTotal;
  const presentTargetIds = new Set(
    solution.breakdown.filter((b) => b.selected && b.level >= 1).map((b) => b.attr_id),
  );
  const missingTargets = (targetAttrs ?? []).filter((a) => !presentTargetIds.has(a.id));
  const missingTitle =
    missingTargets.length > 0
      ? t("card.targetsMissing", {
          names: missingTargets.map((a) => attrName(a.id, a.name)).join(", "),
        })
      : t("card.targetsMissingGeneric");

  return (
    <div className="rounded-xl border border-slate-800 bg-slate-900/50 p-4 transition hover:border-slate-700">
      {/* ヘッダ: ランク + Lv分布 + お気に入り + リンク効果 */}
      <div className="mb-3 flex items-center justify-between">
        <div className="flex items-center gap-2">
          {rank !== undefined && (
            <span
              className={`flex h-6 min-w-6 items-center justify-center rounded-md border px-1.5 text-xs font-bold ${rankColor(
                rank,
              )}`}
            >
              {rank === 1 ? <Trophy size={13} /> : `#${rank}`}
            </span>
          )}
          <span className="flex flex-wrap items-center gap-1.5 text-xs">
            <span className="whitespace-nowrap rounded bg-amber-400/15 px-1.5 py-0.5 font-semibold text-amber-300">
              Lv6 ×{solution.lv6_count}
            </span>
            <span className="whitespace-nowrap rounded bg-sky-400/10 px-1.5 py-0.5 text-sky-300">
              Lv5 ×{solution.lv5_count}
            </span>
            {solution.selected_lv6 > 0 && (
              <span className="flex items-center gap-0.5 whitespace-nowrap rounded bg-emerald-500/15 px-1.5 py-0.5 text-emerald-300">
                <Star size={10} />
                {t("card.selectedLv6", { n: solution.selected_lv6 })}
              </span>
            )}
            {someTargetsMissing && (
              <span
                className="flex cursor-help items-center gap-0.5 whitespace-nowrap rounded bg-amber-500/15 px-1.5 py-0.5 text-amber-300"
                title={missingTitle}
                aria-label={`${t("card.targetsPartial", {
                  n: solution.selected_present,
                  total: targetTotal,
                })} — ${missingTitle}`}
              >
                <TriangleAlert size={10} />
                {t("card.targetsPartial", {
                  n: solution.selected_present,
                  total: targetTotal,
                })}
              </span>
            )}
          </span>
        </div>
        <div className="flex items-center gap-2">
          {onToggleFavorite && (
            <button
              onClick={onToggleFavorite}
              aria-label={isFavorite ? t("card.favRemove") : t("card.favAdd")}
              title={isFavorite ? t("card.favRemove") : t("card.favAdd")}
              aria-pressed={isFavorite}
              className="rounded-md p-1 transition hover:bg-slate-800"
            >
              <Star
                size={16}
                className={
                  isFavorite
                    ? "fill-amber-400 text-amber-400"
                    : "text-slate-500 hover:text-amber-300"
                }
              />
            </button>
          )}
          <div className="text-right leading-none">
            {/* 主表示は常に link_effect（ゲーム内表記と一致する実際のリンク効果）。
                評価に使う eval_link はソフト除外指定時のみ異なりうるため、差がある時だけ
                下に小さく「評価スコア」として併記する（主従を入れ替えないと、ソフト除外を
                使う通常利用で常に「最も目立つ数字がゲーム内と違う値」になってしまうため）。 */}
            <div className="text-2xl font-bold tabular-nums text-slate-100">
              {solution.link_effect}
            </div>
            <div className="text-[10px] text-slate-500">{t("card.linkEffect")}</div>
            {solution.link_effect !== solution.eval_link && (
              <div className="mt-1 text-[10px] text-slate-500">
                {t("card.evalLink")}{" "}
                <span className="font-semibold tabular-nums text-slate-400">
                  {solution.eval_link}
                </span>
              </div>
            )}
          </div>
        </div>
      </div>

      {/* 全属性の内訳（レベル降順） */}
      <div className="mb-3 grid grid-cols-2 gap-x-4 gap-y-1 rounded-lg bg-slate-950/40 p-2.5">
        {solution.breakdown.map((b) => (
          <div
            key={b.attr_id}
            className={`flex items-center justify-between gap-2 text-xs ${
              b.soft_excluded ? "opacity-60" : ""
            }`}
          >
            <span className="flex min-w-0 items-center gap-1">
              {b.selected && <Star size={9} className="shrink-0 text-emerald-400" />}
              <span
                className={`truncate ${
                  b.soft_excluded ? "text-slate-500 line-through" : levelTextColor(b.level, b.selected)
                }`}
                title={attrName(b.attr_id, b.attr_name)}
              >
                {attrName(b.attr_id, b.attr_name)}
              </span>
              {b.soft_excluded && (
                <span className="shrink-0 rounded bg-rose-500/10 px-1 text-[9px] font-medium text-rose-400">
                  {t("card.softExcluded")}
                </span>
              )}
            </span>
            <span className="flex items-center gap-2">
              <span className="text-[10px] text-slate-500">Lv{b.level}</span>
              <LevelDots level={b.level} />
              <span className="w-6 text-right font-semibold tabular-nums text-slate-100">
                {b.value}
              </span>
            </span>
          </div>
        ))}
      </div>

      {/* 構成モジュール一覧 */}
      <div className="grid grid-cols-1 gap-1.5 sm:grid-cols-2">
        {solution.modules.map((m) => (
          <div
            key={m.key}
            className={`rounded-lg border px-2.5 py-1.5 ${qualityColor(m.quality)}`}
          >
            <div className="flex items-center justify-between">
              <span className="truncate text-xs font-semibold">
                {moduleName(m.config_id, m.name)}
              </span>
              <span className="shrink-0 text-[10px] opacity-70">
                {categoryLabel(m.category)}·Q{m.quality}
              </span>
            </div>
            <div className="mt-1 flex flex-wrap gap-1">
              {m.parts.map((p, i) => {
                const hit = targetIds.has(p.attr_id);
                return (
                  <span
                    key={i}
                    className={`rounded px-1.5 py-0.5 text-[10px] ${
                      hit
                        ? "bg-emerald-500/20 font-semibold text-emerald-200"
                        : "bg-slate-800/60 text-slate-400"
                    }`}
                  >
                    {attrName(p.attr_id, p.attr_name)} +{p.value}
                  </span>
                );
              })}
            </div>
          </div>
        ))}
      </div>
    </div>
  );
}
