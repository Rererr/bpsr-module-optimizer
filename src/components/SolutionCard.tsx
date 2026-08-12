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

// モジュール品質 → 配色。ゲーム内の品質色に合わせる（Q4=金 / Q3=紫 / Q2=青）。
// 「候補の絞り込み」の呼称（紫品質以上・金品質以上）と対応させること。
function qualityColor(q: number): string {
  switch (q) {
    case 4:
      return "text-amber-300 border-amber-500/40 bg-amber-500/10";
    case 3:
      return "text-purple-300 border-purple-500/40 bg-purple-500/10";
    case 2:
      return "text-sky-300 border-sky-500/40 bg-sky-500/10";
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
      {/* ヘッダ: 左＝ランクとLv分布バッジ（高さ28pxで統一）、右＝固定幅の数値レール。
          レール幅を固定することで、カードが何枚並んでもリンク効果の数字が縦に揃う。
          比較しながら視線を上下に走らせる画面なので、桁による左右のブレを消すのを優先する。 */}
      <div className="mb-3 flex items-start justify-between gap-3">
        <div className="flex min-w-0 flex-wrap items-center gap-1.5">
          {rank !== undefined && (
            <span
              className={`flex h-7 w-7 shrink-0 items-center justify-center rounded-md border text-xs font-bold tabular-nums ${rankColor(
                rank,
              )}`}
            >
              {rank === 1 ? <Trophy size={14} /> : `#${rank}`}
            </span>
          )}
          {/* Lv6/Lv5 は言語非依存の定型表記なので固定幅にできる。件数が1桁でも2桁でも
              バッジ幅が変わらず、カード間でバッジの切れ目が縦に揃う。 */}
          <span className="flex h-7 w-[74px] shrink-0 items-center justify-center rounded-md bg-amber-400/15 text-xs font-semibold tabular-nums text-amber-300">
            Lv6 ×{solution.lv6_count}
          </span>
          <span className="flex h-7 w-[74px] shrink-0 items-center justify-center rounded-md bg-sky-400/10 text-xs tabular-nums text-sky-300">
            Lv5 ×{solution.lv5_count}
          </span>
          {solution.selected_lv6 > 0 && (
            <span className="flex h-7 items-center gap-1 whitespace-nowrap rounded-md bg-emerald-500/15 px-2 text-xs tabular-nums text-emerald-300">
              <Star size={11} />
              {t("card.selectedLv6", { n: solution.selected_lv6 })}
            </span>
          )}
          {someTargetsMissing && (
            <span
              className="flex h-7 cursor-help items-center gap-1 whitespace-nowrap rounded-md bg-amber-500/15 px-2 text-xs tabular-nums text-amber-300"
              title={missingTitle}
              aria-label={`${t("card.targetsPartial", {
                n: solution.selected_present,
                total: targetTotal,
              })} — ${missingTitle}`}
            >
              <TriangleAlert size={11} />
              {t("card.targetsPartial", {
                n: solution.selected_present,
                total: targetTotal,
              })}
            </span>
          )}
        </div>
        <div className="flex shrink-0 items-start gap-1">
          {onToggleFavorite && (
            <button
              onClick={onToggleFavorite}
              aria-label={isFavorite ? t("card.favRemove") : t("card.favAdd")}
              title={isFavorite ? t("card.favRemove") : t("card.favAdd")}
              aria-pressed={isFavorite}
              className="flex h-7 w-7 items-center justify-center rounded-md transition hover:bg-slate-800"
            >
              <Star
                size={16}
                className={
                  isFavorite
                    ? "fill-amber-400 text-amber-400"
                    : "text-slate-400 hover:text-amber-300"
                }
              />
            </button>
          )}
          {/* 主表示は常に link_effect（ゲーム内表記と一致する実際のリンク効果）。
              評価に使う eval_link はソフト除外指定時のみ異なりうるため、差がある時だけ
              下に小さく「評価スコア」として併記する（主従を入れ替えないと、ソフト除外を
              使う通常利用で常に「最も目立つ数字がゲーム内と違う値」になってしまうため）。
              ラベルと数字は横並び（ベースライン揃え）にして高さをバッジ列と同じ28pxに収める。
              縦積みにすると右端だけが2段分の高さになり、ヘッダーに余白が生まれてしまう。 */}
          <div className="w-[116px] border-l border-slate-800 pl-1.5">
            <div className="flex h-7 items-center justify-end">
              <span className="flex items-baseline gap-1.5">
                <span className="text-[12px] leading-none text-slate-400">
                  {t("card.linkEffect")}
                </span>
                <span className="text-xl font-bold leading-none tabular-nums text-slate-100">
                  {solution.link_effect}
                </span>
              </span>
            </div>
            {solution.link_effect !== solution.eval_link && (
              <div className="mt-0.5 text-right text-[12px] leading-none text-slate-400">
                {t("card.evalLink")}{" "}
                <span className="font-semibold tabular-nums text-slate-300">
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
          // 目標属性の強調は、行頭の★ではなく「左の罫線＋薄い緑背景」で行う。
          // ★は属性名の表示幅を1文字ぶん奪い、長い属性名が先に truncate されてしまうため。
          // 罫線は負のマージンで枠の内側パディングへ逃がし、選択の有無で属性名の開始位置が
          // ずれないようにする（非選択行は同じ幅の透明ボーダー）。
          <div
            key={b.attr_id}
            className={`-ml-2 flex items-center justify-between gap-2 rounded-r border-l-2 pl-1.5 text-xs ${
              b.selected ? "border-emerald-400/70 bg-emerald-500/10" : "border-transparent"
            } ${b.soft_excluded ? "opacity-75" : ""}`}
          >
            <span className="flex min-w-0 items-center gap-1">
              <span
                className={`truncate ${
                  b.soft_excluded ? "text-slate-400 line-through" : levelTextColor(b.level, b.selected)
                }`}
                title={attrName(b.attr_id, b.attr_name)}
              >
                {attrName(b.attr_id, b.attr_name)}
              </span>
              {b.soft_excluded && (
                <span className="shrink-0 rounded bg-rose-500/10 px-1 text-[11px] font-medium text-rose-400">
                  {t("card.softExcluded")}
                </span>
              )}
            </span>
            {/* Lv・ドット・実値の3列を固定幅にして、内訳のどの行も同じ位置で切れるようにする。 */}
            <span className="flex shrink-0 items-center gap-2">
              <span className="w-8 text-right text-[12px] tabular-nums text-slate-400">
                Lv{b.level}
              </span>
              <LevelDots level={b.level} />
              <span className="w-7 text-right font-semibold tabular-nums text-slate-100">
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
              <span className="shrink-0 text-[12px] opacity-90">
                {categoryLabel(m.category)}·Q{m.quality}
              </span>
            </div>
            {/* ここだけは固定幅にしない。モジュール枠は最大3部位が入るには狭く、等幅に
                割ると「集中・攻撃速度」級の属性名が数文字で切れてしまう。属性名は結果を
                読む上で値と同じくらい重要なので、内容幅のまま折り返す。 */}
            <div className="mt-1 flex flex-wrap gap-1">
              {m.parts.map((p, i) => {
                const hit = targetIds.has(p.attr_id);
                const name = attrName(p.attr_id, p.attr_name);
                return (
                  <span
                    key={i}
                    className={`flex items-center gap-1 rounded px-1.5 py-0.5 text-[12px] ${
                      hit
                        ? "bg-emerald-500/20 font-semibold text-emerald-200"
                        : "bg-slate-800/60 text-slate-400"
                    }`}
                  >
                    {name}
                    <span className="tabular-nums">+{p.value}</span>
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
