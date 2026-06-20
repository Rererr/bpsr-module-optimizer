import { X, ArrowUp } from "lucide-react";
import type { SavedBuild } from "../types";
import { BuildFingerprint } from "./BuildFingerprint";

interface Props {
  builds: SavedBuild[]; // 2〜3件
  onClose: () => void;
}

function dotColor(level: number): string {
  return level === 6 ? "bg-amber-400" : level === 5 ? "bg-sky-400" : "bg-emerald-500";
}

function CompareDots({ level }: { level: number }) {
  const color = dotColor(level);
  return (
    <span className="inline-flex gap-0.5">
      {Array.from({ length: 6 }).map((_, i) => (
        <span
          key={i}
          className={`h-1 w-1 rounded-full ${i < level ? color : "bg-slate-700"}`}
        />
      ))}
    </span>
  );
}

// 差がある場合のみ最大値を真とする（全て同値ならハイライトしない）。
function isWinner(values: number[], v: number): boolean {
  const max = Math.max(...values);
  const min = Math.min(...values);
  return max > min && v === max;
}

export function BuildCompare({ builds, onClose }: Props) {
  // 属性の和集合。各ビルドの breakdown を attr_id で索引化する。
  const perBuild = builds.map(
    (b) => new Map(b.solution.breakdown.map((x) => [x.attr_id, x])),
  );
  const nameOf = new Map<number, string>();
  for (const b of builds)
    for (const x of b.solution.breakdown) nameOf.set(x.attr_id, x.attr_name);

  // 最大レベル降順 → attr_id 昇順で属性行を並べる。
  const attrIds = [...nameOf.keys()].sort((a, b) => {
    const la = Math.max(...perBuild.map((m) => m.get(a)?.level ?? 0));
    const lb = Math.max(...perBuild.map((m) => m.get(b)?.level ?? 0));
    return lb - la || a - b;
  });

  const cols = `minmax(84px,0.7fr) repeat(${builds.length}, minmax(0,1fr))`;
  const cell = "border-b border-slate-800 px-2 py-1.5";
  const label = `${cell} text-[11px] text-slate-400`;

  const metric = (
    key: string,
    title: string,
    pick: (b: SavedBuild) => number,
  ) => {
    const vals = builds.map(pick);
    return (
      <>
        <div key={`${key}-l`} className={label}>
          {title}
        </div>
        {builds.map((b, i) => {
          const v = pick(b);
          const win = isWinner(vals, v);
          return (
            <div
              key={`${key}-${i}`}
              className={`${cell} flex items-center gap-1 text-sm tabular-nums ${
                win ? "font-bold text-emerald-300" : "text-slate-200"
              }`}
            >
              {v}
              {win && <ArrowUp size={12} className="text-emerald-400" />}
            </div>
          );
        })}
      </>
    );
  };

  return (
    <div className="rounded-xl border border-slate-700 bg-slate-900/60 p-4">
      <div className="mb-3 flex items-center justify-between">
        <h3 className="text-sm font-bold text-slate-100">
          ビルド比較（{builds.length}件）
        </h3>
        <button
          onClick={onClose}
          className="flex items-center gap-1 rounded-md border border-slate-700 px-2 py-1 text-xs text-slate-300 transition hover:bg-slate-800"
        >
          <X size={13} />
          閉じる
        </button>
      </div>

      <div className="overflow-x-auto">
        <div
          className="grid gap-x-2"
          style={{
            gridTemplateColumns: cols,
            minWidth: 140 + builds.length * 160,
          }}
        >
        {/* ヘッダ行: 名前 + 指紋 */}
        <div className={`${cell} text-[11px] text-slate-500`} />
        {builds.map((b, i) => (
          <div key={`h-${i}`} className={`${cell} flex flex-col gap-1`}>
            <span className="truncate text-xs font-semibold text-slate-100" title={b.name}>
              {b.name}
            </span>
            <BuildFingerprint modules={b.solution.modules} />
          </div>
        ))}

        {metric("link", "リンク効果", (b) => b.solution.link_effect)}
        {metric("lv6", "Lv6数", (b) => b.solution.lv6_count)}
        {metric("lv5", "Lv5数", (b) => b.solution.lv5_count)}

        {/* 属性行（和集合） */}
        {attrIds.map((id) => {
          const levels = perBuild.map((m) => m.get(id)?.level ?? 0);
          return (
            <div key={`a-${id}`} className="contents">
              <div className={`${label} truncate`} title={nameOf.get(id)}>
                {nameOf.get(id)}
              </div>
              {perBuild.map((m, i) => {
                const bd = m.get(id);
                const lv = bd?.level ?? 0;
                const win = isWinner(levels, lv);
                return (
                  <div
                    key={`a-${id}-${i}`}
                    className={`${cell} flex items-center justify-between gap-1.5 ${
                      win ? "rounded bg-emerald-500/10" : ""
                    }`}
                  >
                    <span className="flex items-center gap-1.5">
                      <span
                        className={`text-[10px] tabular-nums ${
                          win ? "font-semibold text-emerald-300" : "text-slate-400"
                        }`}
                      >
                        Lv{lv}
                      </span>
                      <CompareDots level={lv} />
                    </span>
                    <span className="flex items-center gap-0.5 text-[11px] tabular-nums text-slate-300">
                      {bd?.value ?? 0}
                      {win && <ArrowUp size={10} className="text-emerald-400" />}
                    </span>
                  </div>
                );
              })}
            </div>
          );
        })}
        </div>
      </div>
    </div>
  );
}
