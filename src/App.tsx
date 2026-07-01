import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { Sparkles, Loader2, PackageOpen, Star } from "lucide-react";
import {
  captureStatus,
  getAttributes,
  optimize,
  reloadFromDump,
} from "./api";
import type {
  AttrMeta,
  AttrState,
  OptimizeResult,
  SearchPreset,
  StatusDto,
} from "./types";
import { STORAGE_KEYS, loadJSON, saveJSON } from "./storage";
import { useI18n } from "./i18n";
import { usePresets } from "./hooks/usePresets";
import { useFavorites } from "./hooks/useFavorites";
import { StatusBar } from "./components/StatusBar";
import { AttributePicker } from "./components/AttributePicker";
import { RequirementList } from "./components/RequirementList";
import { SolutionCard } from "./components/SolutionCard";
import { PresetBar } from "./components/PresetBar";
import { ConditionSummary } from "./components/ConditionSummary";
import { FavoritesPanel } from "./components/FavoritesPanel";

const CATEGORIES = ["all", "attack", "guardian", "support"];
const TOP_K_OPTIONS = [3, 5, 10];
const SLOT_COUNT_OPTIONS = [4, 5];
// 旧データ（slotCount 欠落）や不正値のフォールバック既定値。
const DEFAULT_SLOT_COUNT = 4;

// 再起動時に復元する検索条件。
interface LastSearch {
  selection: Record<number, AttrState>;
  requireLevels: Record<number, number>;
  category: string;
  topK: number;
  slotCount: number;
}

type Tab = "results" | "favorites";

export default function App() {
  const { t, categoryLabel } = useI18n();
  const [attributes, setAttributes] = useState<AttrMeta[]>([]);

  // 最後の検索条件を1度だけ読み込み、各状態の初期値に使う。
  const [restored] = useState<Partial<LastSearch>>(() =>
    loadJSON<Partial<LastSearch>>(STORAGE_KEYS.lastSearch, {}),
  );
  const [selection, setSelection] = useState<Record<number, AttrState>>(
    () => restored.selection ?? {},
  );
  const [category, setCategory] = useState<string>(() => restored.category ?? "all");
  const [topK, setTopK] = useState<number>(() => restored.topK ?? 5);
  const [slotCount, setSlotCount] = useState<number>(
    () => restored.slotCount ?? DEFAULT_SLOT_COUNT,
  );
  // 属性ごとの下限レベル（attr_id -> level、0/未設定=制約なし）。
  const [requireLevels, setRequireLevels] = useState<Record<number, number>>(
    () => restored.requireLevels ?? {},
  );

  const [tab, setTab] = useState<Tab>("results");
  const [status, setStatus] = useState<StatusDto | null>(null);
  const [result, setResult] = useState<OptimizeResult | null>(null);
  const [loading, setLoading] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const presets = usePresets();
  const favorites = useFavorites();

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

  // 検索条件を localStorage に永続化（再起動時の自動復元用）。
  useEffect(() => {
    saveJSON<LastSearch>(STORAGE_KEYS.lastSearch, {
      selection,
      requireLevels,
      category,
      topK,
      slotCount,
    });
  }, [selection, requireLevels, category, topK, slotCount]);

  const refreshStatus = useCallback(() => {
    captureStatus().then(setStatus).catch(() => {});
  }, []);

  // 最新の探索条件を ref に保持（イベント購読クロージャから参照するため）。
  const runRef = useRef<() => void>(() => {});

  // 明示的に渡した条件で探索する（プリセット適用・条件解除でも再利用）。
  const runWith = useCallback(
    async (
      sel: Record<number, AttrState>,
      reqLevels: Record<number, number>,
      cat: string,
      k: number,
      slots: number,
    ) => {
      setError(null);
      setLoading(true);
      try {
        const tIds = Object.entries(sel)
          .filter(([, s]) => s === "target")
          .map(([id]) => Number(id));
        const eIds = Object.entries(sel)
          .filter(([, s]) => s === "exclude")
          .map(([id]) => Number(id));
        const requirements = tIds
          .map((id) => [id, reqLevels[id] ?? 0] as [number, number])
          .filter(([, lv]) => lv > 0);
        const res = await optimize({
          selectedIds: tIds,
          category: cat === "all" ? null : cat,
          excludeIds: eIds,
          requirements,
          topK: k,
          slotCount: slots,
        });
        setResult(res);
        if (res.solutions.length === 0) {
          setError(
            res.candidate_count < slots
              ? t("error.tooFewCandidates", { c: res.candidate_count, slots })
              : requirements.length > 0
                ? t("error.noReqMatch")
                : t("error.noMatch"),
          );
        }
      } catch (e) {
        setError(String(e));
        setResult(null);
      } finally {
        setLoading(false);
      }
    },
    [t],
  );

  const runOptimize = useCallback(
    () => runWith(selection, requireLevels, category, topK, slotCount),
    [runWith, selection, requireLevels, category, topK, slotCount],
  );

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

  // サマリーバーから目標/除外を1つ解除して即再探索。
  const removeAttr = (id: number) => {
    const next = { ...selection };
    delete next[id];
    setSelection(next);
    runWith(next, requireLevels, category, topK, slotCount);
  };

  const resetCategory = () => {
    setCategory("all");
    runWith(selection, requireLevels, "all", topK, slotCount);
  };

  const applyPreset = (p: SearchPreset) => {
    // 旧プリセットには slotCount が無いため既定値へフォールバック。
    const slots = p.slotCount ?? DEFAULT_SLOT_COUNT;
    setSelection(p.selection);
    setRequireLevels(p.requireLevels);
    setCategory(p.category);
    setTopK(p.topK);
    setSlotCount(slots);
    setTab("results");
    runWith(p.selection, p.requireLevels, p.category, p.topK, slots);
  };

  const savePreset = (name: string) =>
    presets.save(name, { selection, requireLevels, category, topK, slotCount });

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
  const canSave =
    targetIds.length > 0 || excludeIds.length > 0 || category !== "all";
  const favCount = favorites.favorites.length;

  return (
    <div className="flex h-screen flex-col bg-slate-950 text-slate-200">
      <StatusBar
        status={status}
        onReloadDump={onReloadDump}
        busy={busy}
        slotCount={slotCount}
      />

      <div className="flex min-h-0 flex-1">
        {/* 左サイドバー: 条件設定 */}
        <aside className="flex w-[360px] shrink-0 flex-col gap-5 overflow-y-auto border-r border-slate-800 bg-slate-900/30 p-5">
          <section>
            <h2 className="mb-2 text-xs font-bold uppercase tracking-wider text-slate-400">
              {t("section.presets")}
            </h2>
            <PresetBar
              presets={presets.presets}
              onApply={applyPreset}
              onSave={savePreset}
              onDelete={presets.remove}
              canSave={canSave}
            />
          </section>

          <section>
            <h2 className="mb-2 text-xs font-bold uppercase tracking-wider text-slate-400">
              {t("section.attributes")}
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
              {t("section.category")}
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
                  {categoryLabel(c)}
                </button>
              ))}
            </div>
          </section>

          <section>
            <h2 className="mb-2 text-xs font-bold uppercase tracking-wider text-slate-400">
              {t("section.slotCount")}
            </h2>
            <div className="flex gap-1 rounded-lg bg-slate-800/60 p-1">
              {SLOT_COUNT_OPTIONS.map((s) => (
                <button
                  key={s}
                  onClick={() => setSlotCount(s)}
                  className={`flex-1 rounded-md px-2 py-1.5 text-xs font-medium transition ${
                    slotCount === s
                      ? "bg-indigo-500 text-white shadow"
                      : "text-slate-300 hover:bg-slate-700/60"
                  }`}
                >
                  {t("slotCount.option", { n: s })}
                </button>
              ))}
            </div>
          </section>

          <section>
            <h2 className="mb-2 text-xs font-bold uppercase tracking-wider text-slate-400">
              {t("section.minLv")}
            </h2>
            <RequirementList
              targets={targetAttrs}
              levels={requireLevels}
              onChange={setReqLevel}
            />
          </section>

          <section>
            <h2 className="mb-2 text-xs font-bold uppercase tracking-wider text-slate-400">
              {t("section.topK")}
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
                  {t("topK.option", { k })}
                </button>
              ))}
            </div>
          </section>

          <button
            onClick={() => {
              setTab("results");
              runOptimize();
            }}
            disabled={loading || noModules}
            className="mt-auto flex items-center justify-center gap-2 rounded-lg bg-emerald-600 px-4 py-3 text-sm font-bold text-white shadow-lg shadow-emerald-900/30 transition hover:bg-emerald-500 disabled:cursor-not-allowed disabled:bg-slate-700 disabled:text-slate-400 disabled:shadow-none"
          >
            {loading ? (
              <Loader2 size={16} className="animate-spin" />
            ) : (
              <Sparkles size={16} />
            )}
            {t("run")}
          </button>
        </aside>

        {/* メイン: タブ（結果 / お気に入り） */}
        <main className="min-h-0 flex-1 overflow-y-auto p-5">
          <div className="mb-3 flex items-center gap-1 rounded-lg bg-slate-800/60 p-1 w-fit">
            <button
              onClick={() => setTab("results")}
              className={`rounded-md px-3 py-1.5 text-xs font-medium transition ${
                tab === "results"
                  ? "bg-indigo-500 text-white shadow"
                  : "text-slate-300 hover:bg-slate-700/60"
              }`}
            >
              {t("tab.results")}
            </button>
            <button
              onClick={() => setTab("favorites")}
              className={`flex items-center gap-1.5 rounded-md px-3 py-1.5 text-xs font-medium transition ${
                tab === "favorites"
                  ? "bg-indigo-500 text-white shadow"
                  : "text-slate-300 hover:bg-slate-700/60"
              }`}
            >
              <Star
                size={12}
                className={favCount > 0 ? "fill-amber-400 text-amber-400" : ""}
              />
              {t("tab.favorites")}
              {favCount > 0 && (
                <span className="rounded-full bg-slate-900/60 px-1.5 text-[10px] tabular-nums">
                  {favCount}
                </span>
              )}
            </button>
          </div>

          {tab === "results" ? (
            <>
              {error && (
                <div className="mb-4 rounded-lg border border-amber-700/50 bg-amber-500/10 px-4 py-2.5 text-sm text-amber-200">
                  {error}
                </div>
              )}

              <ConditionSummary
                attributes={attributes}
                selection={selection}
                requireLevels={requireLevels}
                category={category}
                onRemoveAttr={removeAttr}
                onResetCategory={resetCategory}
              />

              {result && result.solutions.length > 0 && (
                <div className="mb-3 flex items-center justify-between text-xs text-slate-400">
                  <span>
                    {t("results.summary", {
                      sets: result.solutions.length,
                      candidates: result.candidate_count,
                      combos: result.combinations.toLocaleString(),
                    })}
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
                      isFavorite={favorites.isFavorite(s)}
                      onToggleFavorite={() => favorites.toggle(s)}
                    />
                  ))}
                </div>
              ) : (
                !error && (
                  <div className="flex h-full flex-col items-center justify-center gap-3 text-center text-slate-500">
                    <PackageOpen size={48} className="opacity-40" />
                    {noModules ? (
                      <div>
                        <p className="text-sm">{t("empty.noModulesTitle")}</p>
                        <p className="mt-1 text-xs">{t("empty.noModulesDesc")}</p>
                      </div>
                    ) : (
                      <div>
                        <p className="text-sm">{t("empty.readyTitle")}</p>
                        <p className="mt-1 text-xs">
                          {t("empty.readyDesc", {
                            n: status?.module_count ?? 0,
                            slots: slotCount,
                          })}
                        </p>
                      </div>
                    )}
                  </div>
                )
              )}
            </>
          ) : (
            <FavoritesPanel
              favorites={favorites.favorites}
              onRename={favorites.rename}
              onRemove={favorites.remove}
            />
          )}
        </main>
      </div>
    </div>
  );
}
