import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { Sparkles, Loader2, PackageOpen } from "lucide-react";
import {
  captureStatus,
  getAttributes,
  optimize,
  reloadFromDump,
} from "./api";
import type { AttrMeta, OptimizeResult, StatusDto } from "./types";
import { CATEGORY_LABELS } from "./types";
import { StatusBar } from "./components/StatusBar";
import { AttributePicker, type AttrState } from "./components/AttributePicker";
import { RequirementList } from "./components/RequirementList";
import { SolutionCard } from "./components/SolutionCard";

const CATEGORIES = ["all", "attack", "guardian", "support"];
const TOP_K_OPTIONS = [3, 5, 10];

export default function App() {
  const [attributes, setAttributes] = useState<AttrMeta[]>([]);
  const [selection, setSelection] = useState<Record<number, AttrState>>({});
  const [category, setCategory] = useState<string>("all");
  const [topK, setTopK] = useState<number>(5);
  // 属性ごとの下限レベル（attr_id -> level、0/未設定=制約なし）。
  const [requireLevels, setRequireLevels] = useState<Record<number, number>>({});

  const [status, setStatus] = useState<StatusDto | null>(null);
  const [result, setResult] = useState<OptimizeResult | null>(null);
  const [loading, setLoading] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const targetIds = useMemo(
    () =>
      Object.entries(selection)
        .filter(([, s]) => s === "target")
        .map(([id]) => Number(id)),
    [selection],
  );
  const excludeIds = useMemo(
    () =>
      Object.entries(selection)
        .filter(([, s]) => s === "exclude")
        .map(([id]) => Number(id)),
    [selection],
  );
  const targetSet = useMemo(() => new Set(targetIds), [targetIds]);
  const targetAttrs = useMemo(
    () => attributes.filter((a) => selection[a.id] === "target"),
    [attributes, selection],
  );

  const refreshStatus = useCallback(() => {
    captureStatus().then(setStatus).catch(() => {});
  }, []);

  // 最新の探索条件を ref に保持（イベント購読クロージャから参照するため）。
  const runRef = useRef<() => void>(() => {});

  const runOptimize = useCallback(async () => {
    // 目標未選択でも可（その場合は全体で Lv6数→リンク効果 最大の構成を出す）。
    setError(null);
    setLoading(true);
    try {
      const requirements = targetIds
        .map((id) => [id, requireLevels[id] ?? 0] as [number, number])
        .filter(([, lv]) => lv > 0);
      const res = await optimize({
        selectedIds: targetIds,
        category: category === "all" ? null : category,
        excludeIds,
        requirements,
        topK,
      });
      setResult(res);
      if (res.solutions.length === 0) {
        setError(
          res.candidate_count < 4
            ? `候補モジュールが ${res.candidate_count} 件で4枠に満たません（条件を緩めてください）`
            : requirements.length > 0
              ? "指定した下限Lvをすべて満たす組み合わせがありません（下限を下げるか属性を減らしてください）"
              : "条件に合う組み合わせがありません",
        );
      }
    } catch (e) {
      setError(String(e));
      setResult(null);
    } finally {
      setLoading(false);
    }
  }, [targetIds, excludeIds, category, topK, requireLevels]);

  useEffect(() => {
    runRef.current = runOptimize;
  }, [runOptimize]);

  // 初期化: 属性ロード・ステータス監視・モジュール更新イベント購読。
  useEffect(() => {
    getAttributes().then(setAttributes).catch((e) => setError(String(e)));
    refreshStatus();
    const timer = setInterval(refreshStatus, 2000);
    const un = listen<number>("modules-updated", () => {
      refreshStatus();
      // 目標が選択済みなら新データで自動再探索。
      runRef.current();
    });
    return () => {
      clearInterval(timer);
      un.then((f) => f());
    };
  }, [refreshStatus]);

  const cycle = (id: number) => {
    setSelection((prev) => {
      const cur = prev[id] ?? "none";
      const next: AttrState =
        cur === "none" ? "target" : cur === "target" ? "exclude" : "none";
      const copy = { ...prev };
      if (next === "none") delete copy[id];
      else copy[id] = next;
      return copy;
    });
  };

  const clearSelection = () => {
    setSelection({});
    setRequireLevels({});
  };

  const setReqLevel = (id: number, level: number) =>
    setRequireLevels((prev) => ({ ...prev, [id]: level }));

  const onReloadDump = async () => {
    setBusy(true);
    setError(null);
    try {
      const n = await reloadFromDump();
      refreshStatus();
      if (n > 0 && targetIds.length > 0) runOptimize();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const noModules = (status?.module_count ?? 0) === 0;

  return (
    <div className="flex h-screen flex-col bg-slate-950 text-slate-200">
      <StatusBar status={status} onReloadDump={onReloadDump} busy={busy} />

      <div className="flex min-h-0 flex-1">
        {/* 左サイドバー: 条件設定 */}
        <aside className="flex w-[360px] shrink-0 flex-col gap-5 overflow-y-auto border-r border-slate-800 bg-slate-900/30 p-5">
          <section>
            <h2 className="mb-2 text-xs font-bold uppercase tracking-wider text-slate-400">
              属性を選択
            </h2>
            <AttributePicker
              attributes={attributes}
              selection={selection}
              onCycle={cycle}
              onClear={clearSelection}
            />
          </section>

          <section>
            <h2 className="mb-2 text-xs font-bold uppercase tracking-wider text-slate-400">
              カテゴリ
            </h2>
            <div className="flex gap-1 rounded-lg bg-slate-800/60 p-1">
              {CATEGORIES.map((c) => (
                <button
                  key={c}
                  onClick={() => setCategory(c)}
                  className={`flex-1 rounded-md px-2 py-1.5 text-xs font-medium transition ${
                    category === c
                      ? "bg-indigo-500 text-white shadow"
                      : "text-slate-300 hover:bg-slate-700/60"
                  }`}
                >
                  {CATEGORY_LABELS[c]}
                </button>
              ))}
            </div>
          </section>

          <section>
            <h2 className="mb-2 text-xs font-bold uppercase tracking-wider text-slate-400">
              属性ごとの下限Lv
            </h2>
            <RequirementList
              targets={targetAttrs}
              levels={requireLevels}
              onChange={setReqLevel}
            />
          </section>

          <section>
            <h2 className="mb-2 text-xs font-bold uppercase tracking-wider text-slate-400">
              表示件数
            </h2>
            <div className="flex gap-1 rounded-lg bg-slate-800/60 p-1">
              {TOP_K_OPTIONS.map((k) => (
                <button
                  key={k}
                  onClick={() => setTopK(k)}
                  className={`flex-1 rounded-md px-2 py-1.5 text-xs font-medium transition ${
                    topK === k
                      ? "bg-indigo-500 text-white shadow"
                      : "text-slate-300 hover:bg-slate-700/60"
                  }`}
                >
                  上位{k}
                </button>
              ))}
            </div>
          </section>

          <button
            onClick={runOptimize}
            disabled={loading || noModules}
            className="mt-auto flex items-center justify-center gap-2 rounded-lg bg-emerald-600 px-4 py-3 text-sm font-bold text-white shadow-lg shadow-emerald-900/30 transition hover:bg-emerald-500 disabled:cursor-not-allowed disabled:bg-slate-700 disabled:text-slate-400 disabled:shadow-none"
          >
            {loading ? (
              <Loader2 size={16} className="animate-spin" />
            ) : (
              <Sparkles size={16} />
            )}
            最適化を実行
          </button>
        </aside>

        {/* メイン: 結果 */}
        <main className="min-h-0 flex-1 overflow-y-auto p-5">
          {error && (
            <div className="mb-4 rounded-lg border border-amber-700/50 bg-amber-500/10 px-4 py-2.5 text-sm text-amber-200">
              {error}
            </div>
          )}

          {result && result.solutions.length > 0 && (
            <div className="mb-3 flex items-center justify-between text-xs text-slate-400">
              <span>
                上位 {result.solutions.length} セット
                <span className="text-slate-600"> / </span>
                候補 {result.candidate_count} 件から{" "}
                {result.combinations.toLocaleString()} 通りを探索
              </span>
            </div>
          )}

          {result && result.solutions.length > 0 ? (
            <div className="grid grid-cols-1 gap-3 lg:grid-cols-2 2xl:grid-cols-3">
              {result.solutions.map((s, i) => (
                <SolutionCard
                  key={i}
                  solution={s}
                  rank={i + 1}
                  targetIds={targetSet}
                />
              ))}
            </div>
          ) : (
            !error && (
              <div className="flex h-full flex-col items-center justify-center gap-3 text-center text-slate-500">
                <PackageOpen size={48} className="opacity-40" />
                {noModules ? (
                  <div>
                    <p className="text-sm">所持モジュール未取得</p>
                    <p className="mt-1 text-xs">
                      管理者権限でゲームのマップ移動 or「ダンプ再読込」で取得してください
                    </p>
                  </div>
                ) : (
                  <div>
                    <p className="text-sm">
                      目標属性を選んで「最適化を実行」を押してください
                    </p>
                    <p className="mt-1 text-xs">
                      所持 {status?.module_count ?? 0} 件から最良の4枠を探索します
                    </p>
                  </div>
                )}
              </div>
            )
          )}
        </main>
      </div>
    </div>
  );
}
