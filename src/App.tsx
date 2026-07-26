import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { Sparkles, Loader2, PackageOpen, Star } from "lucide-react";
import {
  captureStatus,
  getAttributes,
  optimize,
  reloadFromDump,
  selectCharacter,
} from "./api";
import type {
  AttrMeta,
  AttrState,
  OptimizeResult,
  RankMode,
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
import { Footer } from "./components/Footer";

const CATEGORIES = ["all", "attack", "guardian", "support"];
const TOP_K_OPTIONS = [3, 5, 10];
const SLOT_COUNT_OPTIONS = [4, 5];
const RANK_MODE_OPTIONS: RankMode[] = ["link", "lv5"];

// localStorage 由来の値は型が保証されない（壊れた値・将来バージョンの値が入りうる）。
// RankMode は Rust 側で厳格な enum のため、不正な値をそのまま渡すと optimize コマンド全体が
// serde デシリアライズエラーで失敗する（topK/slotCount はサーバ側で clamp されるのと対照的）。
// 未知の値は黙って既定の "link" へフォールバックする。
function normalizeRankMode(mode: RankMode | undefined): RankMode {
  return mode !== undefined && RANK_MODE_OPTIONS.includes(mode) ? mode : "link";
}
// 新規インストール時、および slotCount 導入前の旧 lastSearch を復元した時の初期スロット数。
// 前向きに5枠を既定にする（restored.slotCount ?? INITIAL_SLOT_COUNT）。
const INITIAL_SLOT_COUNT = 5;
// slotCount 導入前（＝4枠時代）の旧プリセットを適用する時だけのフォールバック値（applyPreset 専用）。
// 当時の枠数を保つため後方互換で4枠のまま維持する。新規/旧lastSearch の初期値は INITIAL_SLOT_COUNT。
const DEFAULT_SLOT_COUNT = 4;

// 再起動時に復元する検索条件。
interface LastSearch {
  selection: Record<number, AttrState>;
  requireLevels: Record<number, number>;
  category: string;
  topK: number;
  slotCount: number;
  hardExclude: boolean;
  rankMode: RankMode;
}

type Tab = "results" | "favorites";

// 処理時間を人間可読へ整形（1秒未満は ms、それ以上は秒2桁）。
function formatElapsed(ms: number): string {
  return ms < 1000 ? `${Math.round(ms)} ms` : `${(ms / 1000).toFixed(2)} s`;
}

// 実際に使われた探索エンジンの表記。"(GPU)"/"(CPU)" は言語非依存の表記とし ja/en 共通。
function formatEngine(engine: string): string {
  return `(${engine.toUpperCase()})`;
}

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
    () => restored.slotCount ?? INITIAL_SLOT_COUNT,
  );
  // 属性ごとの下限レベル（attr_id -> level、0/未設定=制約なし）。
  const [requireLevels, setRequireLevels] = useState<Record<number, number>>(
    () => restored.requireLevels ?? {},
  );
  // 除外モード: true=ハード除外（該当モジュールを丸ごと除外）、false=ソフト除外（新規既定。
  // 属性のみランキング集計から除外し、モジュール自体は候補に残す）。
  // 旧データ（本機能追加前）は除外＝常にハード除外だったため、hardExclude 未定義時は
  // true にフォールバックして従来の挙動を保つ（新規保存時は必ず明示するため ?? は効かない）。
  const [hardExclude, setHardExclude] = useState<boolean>(() => restored.hardExclude ?? true);
  // ランキング順序モード。旧データ（本機能追加前）には存在しないため "link"（既定・従来の
  // 唯一の順序）にフォールバックする。モードを変えただけでは自動再探索しない
  // （category/slotCount/topK と同じ既存パターン。ユーザーが「最適化を実行」を押す想定）。
  const [rankMode, setRankMode] = useState<RankMode>(() =>
    normalizeRankMode(restored.rankMode),
  );
  // 直近に成功した検索で実際に使われた並び順モード。`rankMode`（UIの現在の選択）と
  // ずれたら「表示中の結果は選択中のモードを反映していない」ことを示すために使う
  // （rankMode は topK/slotCount と違って見た目が変わらないため、既存パターンの
  // 「変更だけでは再検索しない」だけでは変更が反映されていないことに気づけない）。
  const [appliedRankMode, setAppliedRankMode] = useState<RankMode | null>(null);
  // フッター表示設定。設定画面が無いため lastSearch とは別キーで独立永続化する（既定=表示ON）。
  const [footerVisible, setFooterVisible] = useState<boolean>(() =>
    loadJSON(STORAGE_KEYS.footerVisible, true),
  );

  const [tab, setTab] = useState<Tab>("results");
  const [status, setStatus] = useState<StatusDto | null>(null);
  const [result, setResult] = useState<OptimizeResult | null>(null);
  // 直近の探索の処理時間（ミリ秒、フロント往復計測）。結果サマリーに表示する。
  const [elapsedMs, setElapsedMs] = useState<number | null>(null);
  const [loading, setLoading] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const presets = usePresets();
  // お気に入りのみキャラ別（uuid は特定キャラの所持モジュールを指すため）。キャラが
  // まだ1件も検知されていない（status 未取得 or characters が空）間は null を渡し、
  // useFavorites 側で読込・保存自体を抑止する（"0" 固定にすると未検知中に
  // bpsr.favorites.v1.0 のゴミキーができてしまうため）。
  const activeCharId = status && status.characters.length > 0 ? status.selected_char_id : null;
  const favorites = useFavorites(activeCharId);

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
      hardExclude,
      rankMode,
    });
  }, [selection, requireLevels, category, topK, slotCount, hardExclude, rankMode]);

  useEffect(() => {
    saveJSON(STORAGE_KEYS.footerVisible, footerVisible);
  }, [footerVisible]);

  // 保存された選択キャラの復元が既に適用済み（またはそもそも保存値が無く不要）かどうか。
  // 一度成立したら、以後の characters-updated / ポーリングで再試行しない
  // （ユーザーの明示選択を先着自動採用で上書きしないため。保存値の書込は
  // onSelectCharacter による明示選択時のみ行う＝復元側は保存値を上書きしない）。
  const restoredSelectionRef = useRef(false);

  // 保存された選択キャラがまだ検知一覧に現れていなければ何もしない（保持したまま
  // 次回の characters-updated で再試行する）。現れていれば select_character で復元する。
  // st.selected_char_id は自動解決の結果であって「明示的に固定された」ことを意味しない
  // （backend の selected_char_id: Option<i64> が None のままでも、たまたま自動解決が
  // 保存値と同じ id を指すことがある）ため、id が一致していても必ず select_character を
  // 呼んで明示的に固定する（同一 id への再選択は冪等）。
  const tryRestoreSelectedCharacter = useCallback((st: StatusDto) => {
    if (restoredSelectionRef.current) return;
    const savedCharId = loadJSON<string | null>(STORAGE_KEYS.selectedCharId, null);
    if (savedCharId == null) {
      restoredSelectionRef.current = true;
      return;
    }
    if (!st.characters.some((c) => c.char_id === savedCharId)) return;
    restoredSelectionRef.current = true;
    // 他キャラの結果を残さない（onSelectCharacter と同じリセットを通す）。
    setResult(null);
    setElapsedMs(null);
    setAppliedRankMode(null);
    setError(null);
    selectCharacter(savedCharId)
      .then(() => captureStatus().then(setStatus).catch(() => {}))
      .catch((e) => setError(String(e)));
  }, []);

  const refreshStatus = useCallback(() => {
    captureStatus()
      .then((st) => {
        setStatus(st);
        tryRestoreSelectedCharacter(st);
      })
      .catch(() => {});
  }, [tryRestoreSelectedCharacter]);

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
      hardExcl: boolean,
      mode: RankMode,
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
        // hardExcl トグルにより、除外指定属性をハード/ソフトいずれか一方へ振り分ける。
        const hardExcludeIds = hardExcl ? eIds : [];
        const softExcludeIds = hardExcl ? [] : eIds;
        // 明示的に下限Lvが設定された目標のみ必須要求として渡す。下限Lv未指定の目標は
        // バックエンドのランキング（選択属性の存在数を最優先で最大化）でソフトに優先される
        // ため、含められる場合は自然に結果へ入り、含められない場合は黙って除外される。
        const requirements = tIds
          .map((id) => [id, reqLevels[id] ?? 0] as [number, number])
          .filter(([, lv]) => lv > 0);
        const startedAt = performance.now();
        const res = await optimize({
          selectedIds: tIds,
          category: cat === "all" ? null : cat,
          excludeIds: hardExcludeIds,
          softExcludeIds,
          requirements,
          topK: k,
          slotCount: slots,
          rankMode: mode,
        });
        setElapsedMs(performance.now() - startedAt);
        setResult(res);
        setAppliedRankMode(mode);
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
        setElapsedMs(null);
      } finally {
        setLoading(false);
      }
    },
    [t],
  );

  const runOptimize = useCallback(
    () => runWith(selection, requireLevels, category, topK, slotCount, hardExclude, rankMode),
    [runWith, selection, requireLevels, category, topK, slotCount, hardExclude, rankMode],
  );

  useEffect(() => {
    runRef.current = runOptimize;
  }, [runOptimize]);

  // 初期化: 属性ロード・ステータス監視・モジュール更新イベント購読。
  // 保存された選択キャラの復元は refreshStatus 経由の tryRestoreSelectedCharacter が
  // 都度（初回・ポーリング・characters-updated）試みる。
  useEffect(() => {
    getAttributes().then(setAttributes).catch((e) => setError(String(e)));
    refreshStatus();

    const timer = setInterval(refreshStatus, 2000);
    const unModules = listen<number>("modules-updated", () => {
      refreshStatus();
      // 新データで自動再探索する（select_character も切替の都度これを emit するため、
      // 目標未選択でも走る＝毎回 runOptimize が呼ばれる。目標未選択時に候補0件などで
      // 空振りするのは runWith 内のエラーハンドリングに委ねる）。
      runRef.current();
    });
    // キャラ一覧（新規検知・名前・件数）が変わったら選択 UI を更新し、保存された
    // 選択キャラがまだ現れていなければ復元を再試行する。
    const unCharacters = listen("characters-updated", () => {
      refreshStatus();
    });
    return () => {
      clearInterval(timer);
      unModules.then((f) => f());
      unCharacters.then((f) => f());
    };
  }, [refreshStatus]);

  // ユーザーの明示選択。保存値の上書きはここだけで行う（復元側は保存値を書き換えない）。
  // status?.selected_char_id は自動解決の結果に過ぎず「既に固定済み」を意味しない
  // ため、一致していても早期リターンせず必ず select_character を呼んで明示的に固定する
  // （さもないと「自動採用中のキャラを明示的に選び直す」操作が永続化されない）。
  const onSelectCharacter = async (charId: string) => {
    if (!charId) return;
    restoredSelectionRef.current = true;
    // 他キャラのモジュールで出した解を残さない。
    setResult(null);
    setElapsedMs(null);
    setAppliedRankMode(null);
    setError(null);
    try {
      await selectCharacter(charId);
      saveJSON(STORAGE_KEYS.selectedCharId, charId);
      // select_character が modules-updated を emit するため、購読側で
      // refreshStatus + 自動再探索が走る。ここでも即時反映のため軽く呼んでおく。
      refreshStatus();
    } catch (e) {
      setError(String(e));
    }
  };

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
    runWith(next, requireLevels, category, topK, slotCount, hardExclude, rankMode);
  };

  const resetCategory = () => {
    setCategory("all");
    runWith(selection, requireLevels, "all", topK, slotCount, hardExclude, rankMode);
  };

  const applyPreset = (p: SearchPreset) => {
    // 旧プリセットには slotCount が無いため既定値へフォールバック。
    const slots = p.slotCount ?? DEFAULT_SLOT_COUNT;
    // 旧プリセット（本機能追加前）は除外＝常にハード除外だったため、hardExclude 未定義時は
    // true にフォールバックして従来の挙動を保つ（新規保存時は常に明示される）。
    const hardExcl = p.hardExclude ?? true;
    // 旧プリセット（案A追加前）には rankMode が無いため、従来の唯一の順序 "link" へ
    // フォールバックする。壊れた値・未知の値も同様に "link" へ正規化する（S5）。
    const mode = normalizeRankMode(p.rankMode);
    setSelection(p.selection);
    setRequireLevels(p.requireLevels);
    setCategory(p.category);
    setTopK(p.topK);
    setSlotCount(slots);
    setHardExclude(hardExcl);
    setRankMode(mode);
    setTab("results");
    runWith(p.selection, p.requireLevels, p.category, p.topK, slots, hardExcl, mode);
  };

  const savePreset = (name: string) =>
    presets.save(name, {
      selection,
      requireLevels,
      category,
      topK,
      slotCount,
      hardExclude,
      rankMode,
    });

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
  // 表示中の結果が、現在選択中の並び順モードを反映していないか（W2）。
  // 初回検索前（appliedRankMode===null）は「古い結果」自体が無いので対象外。
  const isRankModeStale = appliedRankMode !== null && rankMode !== appliedRankMode;

  return (
    <div className="flex h-screen flex-col bg-slate-950 text-slate-200">
      <StatusBar
        status={status}
        onReloadDump={onReloadDump}
        busy={busy}
        slotCount={slotCount}
        footerVisible={footerVisible}
        onShowFooter={() => setFooterVisible(true)}
        onSelectCharacter={onSelectCharacter}
      />

      <div className="flex min-h-0 flex-1">
        {/* 左サイドバー: 条件設定 */}
        {/* 文字サイズを一段上げたぶん、属性チップの折返しが増えないよう幅も少し広げる。 */}
        <aside className="flex w-[384px] shrink-0 flex-col gap-5 overflow-y-auto border-r border-slate-800 bg-slate-900/30 p-5">
          <section>
            <h2 className="mb-2 text-xs font-bold uppercase tracking-wider text-slate-300">
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
            <h2 className="mb-2 text-xs font-bold uppercase tracking-wider text-slate-300">
              {t("section.attributes")}
            </h2>
            <AttributePicker
              attributes={attributes}
              selection={selection}
              onCycle={cycle}
              onClear={clearSelection}
              hardExclude={hardExclude}
              onHardExcludeChange={setHardExclude}
            />
          </section>

          <section>
            <h2 className="mb-2 text-xs font-bold uppercase tracking-wider text-slate-300">
              {t("section.category")}
            </h2>
            <div className="flex gap-1 rounded-lg bg-slate-800/60 p-1">
              {CATEGORIES.map((c) => (
                <button
                  key={c}
                  onClick={() => setCategory(c)}
                  className={`flex h-8 flex-1 items-center justify-center rounded-md text-xs font-medium transition ${
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
            <h2 className="mb-2 text-xs font-bold uppercase tracking-wider text-slate-300">
              {t("section.slotCount")}
            </h2>
            <div className="flex gap-1 rounded-lg bg-slate-800/60 p-1">
              {SLOT_COUNT_OPTIONS.map((s) => (
                <button
                  key={s}
                  onClick={() => setSlotCount(s)}
                  className={`flex h-8 flex-1 items-center justify-center rounded-md text-xs font-medium transition ${
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
            <h2 className="mb-2 text-xs font-bold uppercase tracking-wider text-slate-300">
              {t("section.minLv")}
            </h2>
            <RequirementList
              targets={targetAttrs}
              levels={requireLevels}
              onChange={setReqLevel}
            />
          </section>

          <section>
            <h2 className="mb-2 text-xs font-bold uppercase tracking-wider text-slate-300">
              {t("section.topK")}
            </h2>
            <div className="flex gap-1 rounded-lg bg-slate-800/60 p-1">
              {TOP_K_OPTIONS.map((k) => (
                <button
                  key={k}
                  onClick={() => setTopK(k)}
                  className={`flex h-8 flex-1 items-center justify-center rounded-md text-xs font-medium transition ${
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

          <section>
            <h2 className="mb-2 text-xs font-bold uppercase tracking-wider text-slate-300">
              {t("section.rankMode")}
            </h2>
            <div className="flex gap-1 rounded-lg bg-slate-800/60 p-1">
              {RANK_MODE_OPTIONS.map((m) => (
                <button
                  key={m}
                  onClick={() => setRankMode(m)}
                  className={`flex h-8 flex-1 items-center justify-center rounded-md text-xs font-medium transition ${
                    rankMode === m
                      ? "bg-indigo-500 text-white shadow"
                      : "text-slate-300 hover:bg-slate-700/60"
                  }`}
                >
                  {t(`rankMode.${m}`)}
                </button>
              ))}
            </div>
            <p
              className={`mt-1.5 text-xs leading-relaxed ${
                isRankModeStale ? "text-amber-400" : "text-slate-400"
              }`}
            >
              {isRankModeStale ? t("rankMode.staleHint") : t("rankMode.hint")}
            </p>
          </section>

          <button
            onClick={() => {
              setTab("results");
              runOptimize();
            }}
            disabled={loading || noModules}
            className="mt-auto flex h-11 shrink-0 items-center justify-center gap-2 rounded-lg bg-emerald-600 text-sm font-bold text-white shadow-lg shadow-emerald-900/30 transition hover:bg-emerald-500 disabled:cursor-not-allowed disabled:bg-slate-700 disabled:text-slate-300 disabled:shadow-none"
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
              className={`flex h-8 w-[128px] items-center justify-center rounded-md text-xs font-medium transition ${
                tab === "results"
                  ? "bg-indigo-500 text-white shadow"
                  : "text-slate-300 hover:bg-slate-700/60"
              }`}
            >
              {t("tab.results")}
            </button>
            <button
              onClick={() => setTab("favorites")}
              className={`flex h-8 w-[128px] items-center justify-center gap-1.5 rounded-md text-xs font-medium transition ${
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
                <span className="rounded-full bg-slate-900/60 px-1.5 text-[12px] tabular-nums">
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
                <div className="mb-3 flex items-center justify-between gap-2 text-xs text-slate-400">
                  <span>
                    {t("results.summary", {
                      sets: result.solutions.length,
                      candidates: result.candidate_count,
                      combos: result.combinations.toLocaleString(),
                    })}
                    {appliedRankMode && (
                      <span className={isRankModeStale ? "text-amber-400" : undefined}>
                        {" "}
                        ・
                        {t("results.rankMode", {
                          mode: t(`rankMode.${appliedRankMode}`),
                        })}
                        {isRankModeStale && t("results.rankModeStale")}
                      </span>
                    )}
                  </span>
                  {elapsedMs !== null && (
                    <span className="shrink-0 tabular-nums text-slate-400">
                      {t("results.elapsed", { t: formatElapsed(elapsedMs) })}{" "}
                      {formatEngine(result.engine)}
                    </span>
                  )}
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
                      targetAttrs={targetAttrs}
                      isFavorite={favorites.isFavorite(s)}
                      onToggleFavorite={() => favorites.toggle(s)}
                    />
                  ))}
                </div>
              ) : (
                !error && (
                  <div className="flex h-full flex-col items-center justify-center gap-3 text-center text-slate-400">
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

      {footerVisible && <Footer onHide={() => setFooterVisible(false)} />}
    </div>
  );
}
