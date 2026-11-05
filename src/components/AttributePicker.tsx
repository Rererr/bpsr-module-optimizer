import type { AttrMeta, AttrState } from "../types";
import { useI18n } from "../i18n";
import { HelpPopover } from "./HelpPopover";
import { AttrIcon } from "./GameIcon";

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
// 除外モードのセグメント順（false=ソフト除外が既定なので先頭）。
const EXCLUDE_MODES = [false, true];

function chipClass(state: AttrState, special: boolean): string {
  if (state === "target")
    return "border-emerald-500 bg-emerald-500/15 text-emerald-200 ring-1 ring-emerald-500/40";
  if (state === "exclude")
    return "border-rose-500 bg-rose-500/10 text-rose-300 line-through ring-1 ring-rose-500/30";
  return special
    ? "border-amber-600/60 bg-amber-500/10 text-amber-200 hover:border-amber-500"
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
          {/* 使い方の説明は常時表示すると、属性チップと除外モードの切替を画面下へ押し出す。
              説明の中身は「除外」の意味に踏み込む部分が大半なので、除外ラベルの隣へ置く。 */}
          <HelpPopover label={t("picker.helpLabel")}>
            {lang === "ja" ? (
              <>
                <p>
                  クリックで <span className="font-semibold text-emerald-300">目標</span> →{" "}
                  <span className="font-semibold text-rose-300">除外</span> → 解除 と切り替わります。
                </p>
                <dl className="mt-2 space-y-1.5">
                  <div>
                    <dt className="font-semibold text-emerald-300">目標</dt>
                    <dd>その属性のLv6到達を優先します。</dd>
                  </div>
                  <div>
                    <dt className="font-semibold text-rose-300">除外（ソフト・既定）</dt>
                    <dd>
                      その属性をスコア評価から外します。Lv6/Lv5の数にもリンク効果にも数えません。
                      同点になったときだけ、巻き添えの少ない方を選びます。
                    </dd>
                  </div>
                  <div>
                    <dt className="font-semibold text-rose-300">除外（ハード）</dt>
                    <dd>下の切替を「ハード除外」にすると、その属性を含むモジュールを一切使いません。</dd>
                  </div>
                </dl>
              </>
            ) : (
              <>
                <p>
                  Click to cycle <span className="font-semibold text-emerald-300">Target</span> →{" "}
                  <span className="font-semibold text-rose-300">Exclude</span> → off.
                </p>
                <dl className="mt-2 space-y-1.5">
                  <div>
                    <dt className="font-semibold text-emerald-300">Target</dt>
                    <dd>Prioritizes reaching Lv6 for that attribute.</dd>
                  </div>
                  <div>
                    <dt className="font-semibold text-rose-300">Exclude (soft, default)</dt>
                    <dd>
                      Drops the attribute from scoring — not counted toward Lv6/Lv5 totals or the
                      link effect. Only breaks ties, in favor of the build carrying less of it.
                    </dd>
                  </div>
                  <div>
                    <dt className="font-semibold text-rose-300">Exclude (hard)</dt>
                    <dd>
                      Set the control below to Hard exclude to never use modules containing that
                      attribute.
                    </dd>
                  </div>
                </dl>
              </>
            )}
          </HelpPopover>
        </div>
        <button
          onClick={onClear}
          className="text-xs text-slate-400 underline-offset-2 hover:text-slate-200 hover:underline"
        >
          {t("picker.clear")}
        </button>
      </div>

      {/* 除外の強さは、チェックボックス（オフ側の意味が読み取れない）ではなく
          2択セグメントにする。他の設定と同じ形なので、開かなくても現在どちらで
          探索するかが分かる。 */}
      <div className="flex gap-1 rounded-lg bg-slate-800/60 p-1">
        {EXCLUDE_MODES.map((hard) => (
          <button
            key={String(hard)}
            onClick={() => onHardExcludeChange(hard)}
            aria-pressed={hardExclude === hard}
            className={`flex h-8 flex-1 items-center justify-center rounded-md text-xs font-medium transition ${
              hardExclude === hard
                ? "bg-indigo-500 text-white shadow"
                : "text-slate-300 hover:bg-slate-700/60"
            }`}
          >
            {t(hard ? "picker.excludeHard" : "picker.excludeSoft")}
          </button>
        ))}
      </div>

      <div className="flex flex-col gap-3">
        {groups.map((g) => (
          <div key={g}>
            <div className="mb-1.5 text-xs font-semibold uppercase tracking-wider text-slate-400">
              {groupLabel(g)}
            </div>
            {/* 属性名の文字数がまちまちなため、可変幅のチップを折り返すと右端がガタつく。
                2列の等幅グリッドに固定し、グループが変わっても列位置が揃うようにする。 */}
            <div className="grid grid-cols-2 gap-1.5">
              {byGroup.get(g)!.map((a) => {
                const st = selection[a.id] ?? "none";
                const name = attrName(a.id, a.name);
                return (
                  <button
                    key={a.id}
                    onClick={() => onCycle(a.id)}
                    title={name}
                    className={`flex h-8 items-center gap-1.5 rounded-md border px-2.5 text-xs font-medium transition ${chipClass(
                      st,
                      a.special,
                    )}`}
                  >
                    <AttrIcon
                      attrId={a.id}
                      size={16}
                      className={st === "exclude" ? "opacity-50 grayscale" : undefined}
                    />
                    <span className="truncate">{name}</span>
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
