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
import { ValueIssueBanner } from "./components/ValueIssueBanner";
import { AttributePicker } from "./components/AttributePicker";
import { RequirementList } from "./components/RequirementList";
import { SolutionCard } from "./components/SolutionCard";
import { PresetBar } from "./components/PresetBar";
import { ConditionSummary } from "./components/ConditionSummary";
import { FavoritesPanel } from "./components/FavoritesPanel";
import { Footer } from "./components/Footer";
import { CutoffWarningDialog } from "./components/CutoffWarningDialog";

const CATEGORIES = ["all", "attack", "guardian", "support"];
// 表示件数の入力範囲。上限50は「GPU カーネルが扱える top_k の上限64（超えると CPU
// フォールバックで遅くなる）」と「バックエンドの clamp(1,100)」の内側に収まる値。
const TOP_K_MIN = 1;
const TOP_K_MAX = 50;
const DEFAULT_TOP_K = 5;
const SLOT_COUNT_OPTIONS = [4, 5];
const RANK_MODE_OPTIONS: RankMode[] = ["link", "lv5"];
// 候補の切り捨て（「候補の絞り込み」）。速度目的ではなくユーザーの明示的な意思表示
// （このモジュールは使わない）として提供するため、既定は常に無効＝所持すべてが候補。
// 上限は src-tauri/src/lib.rs の MAX_MIN_QUALITY / MAX_MIN_MODULE_VALUE と一致させること。
// 上限の根拠: 512件×5パターンの実測で、合計値 8 までは全パターンで上位10件が全数探索と
// 一致した（10〜12 で2位以下が、13 以上で1位が崩れる）。
const CUTOFF_MAX_QUALITY = 4;
const CUTOFF_MAX_VALUE = 8;
// 品質の選択肢（0=絞り込みなし）。実データの品質は 2(部位1)/3(部位2)/4(部位3)。
const MIN_QUALITY_OPTIONS = [0, 3, 4];

// localStorage 由来の値は型が保証されない（壊れた値・将来バージョンの値が入りうる）。
// RankMode は Rust 側で厳格な enum のため、不正な値をそのまま渡すと optimize コマンド全体が
// serde デシリアライズエラーで失敗する（topK/slotCount はサーバ側で clamp されるのと対照的）。
// 未知の値は黙って既定の "link" へフォールバックする。
function normalizeRankMode(mode: RankMode | undefined): RankMode {
  return mode !== undefined && RANK_MODE_OPTIONS.includes(mode) ? mode : "link";
}
// 表示件数の正規化。localStorage・プリセット由来の値は型も範囲も保証されない
// （旧バージョンで保存した選択肢、手で壊された値、上限変更前の値が入りうる）ため、
// 整数へ丸めて TOP_K_MIN〜TOP_K_MAX に収める。バックエンドも clamp(1,100) するが、
// 入力欄の表示・保存値・探索へ渡す値が食い違わないよう、フロント側の正規化口はここ1つ。
function normalizeTopK(v: number | undefined): number {
  if (typeof v !== "number" || !Number.isFinite(v)) return DEFAULT_TOP_K;
  return Math.min(Math.max(Math.round(v), TOP_K_MIN), TOP_K_MAX);
}
// 切り捨て設定の正規化。旧データ（本機能追加前の lastSearch / プリセット）には存在せず、
// 壊れた値も入りうるため、未定義・範囲外は 0（絞り込みなし）へ寄せる。
function normalizeCutoff(v: number | undefined, max: number): number {
  if (typeof v !== "number" || !Number.isFinite(v)) return 0;
  return Math.min(Math.max(Math.round(v), 0), max);
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
  minQuality: number;
  minModuleValue: number;
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
  const [topK, setTopK] = useState<number>(() => normalizeTopK(restored.topK));
  // 表示件数の入力欄に表示している文字列。入力途中の未確定値（空欄、"50" を打つ
  // 途中の "5"、上限超えの "60"）を許容するため確定値 topK とは別に持ち、
  // フォーカスが外れた時点（または Enter）で正規化して両者を揃える。
  const [topKDraft, setTopKDraft] = useState<string>(() =>
    String(normalizeTopK(restored.topK)),
  );
  const [slotCount, setSlotCount] = useState<number>(
    () => restored.slotCount ?? INITIAL_SLOT_COUNT,
  );
  // 属性ごとの下限レベル（attr_id -> level、0/未設定=制約なし）。
  const [requireLevels, setRequireLevels] = useState<Record<number, number>>(
    () => restored.requireLevels ?? {},
  );
  // 候補の切り捨て（0=絞り込みなし）。既定は無効で、有効化時に確認ダイアログを出す。
  const [minQuality, setMinQuality] = useState<number>(() =>
    normalizeCutoff(restored.minQuality, CUTOFF_MAX_QUALITY),
  );
  const [minModuleValue, setMinModuleValue] = useState<number>(() =>
    normalizeCutoff(restored.minModuleValue, CUTOFF_MAX_VALUE),
  );
  // 合計値の入力欄の表示文字列（topKDraft と同じ理由で確定値とは別に持つ）。
  const [minValueDraft, setMinValueDraft] = useState<string>(() =>
    String(normalizeCutoff(restored.minModuleValue, CUTOFF_MAX_VALUE)),
  );
  // 確認待ちの切り捨て設定（null=確認ダイアログ非表示）。無効→有効の変更時のみ入る。
  const [pendingCutoff, setPendingCutoff] = useState<{
    quality: number;
    value: number;
  } | null>(null);
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
  // 所持モジュールが更新されたのに自動再探索をスキップした（＝表示中の結果が現在の
  // 所持モジュールより古い）ことを示す。探索が成功した時点で解除する。
  const [modulesStale, setModulesStale] = useState(false);
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
      minQuality,
      minModuleValue,
    });
  }, [
    selection,
    requireLevels,
    category,
    topK,
    slotCount,
    hardExclude,
    rankMode,
    minQuality,
    minModuleValue,
  ]);

  useEffect(() => {
    saveJSON(STORAGE_KEYS.footerVisible, footerVisible);
  }, [footerVisible]);

  // 保存された選択キャラの復元が既に適用済み（またはそもそも保存値が無く不要）かどうか。
  // 一度成立したら、以後の characters-updated / ポーリングで再試行しない
  // （ユーザーの明示選択を先着自動採用で上書きしないため。保存値の書込は
  // onSelectCharacter による明示選択時のみ行う＝復元側は保存値を上書きしない）。
  const restoredSelectionRef = useRef(false);

  // 探索の世代。結果を片付けた（キャラ切替・0 件化）後に、それ以前に始めた探索が
  // 完了して古い結果を書き戻さないよう、開始時の世代と一致する時だけ反映する。
  const searchGenRef = useRef(0);
  // 実行中の探索数。バックエンドの探索は中断できないので、結果を捨てた探索も終わるまで
  // 実行中表示（＝実行ボタン無効）を保ち、手動では重い探索を並べて走らせない
  // （modules-updated からの自動再探索は loading を見ないので重なりうる）。
  const inflightRef = useRef(0);
  // 「結果が古い」を立てた時点の世代。それ以前に始めた探索の完了では印を下ろさない
  // （変更前のモジュールで出した結果なので古いまま）。
  const staleSinceGenRef = useRef(-1);

  // 結果を片付ける唯一の入口（キャラ切替・保存選択の復元・0 件化）。
  const discardResults = useCallback(() => {
    searchGenRef.current += 1;
    setResult(null);
    setElapsedMs(null);
    setAppliedRankMode(null);
    setError(null);
  }, []);

  const markModulesStale = useCallback(() => {
    staleSinceGenRef.current = searchGenRef.current;
    setModulesStale(true);
  }, []);

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
    discardResults();
    selectCharacter(savedCharId)
      .then(() => captureStatus().then(setStatus).catch(() => {}))
      .catch((e) => setError(String(e)));
  }, [discardResults]);

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
  // 目標属性が選ばれているか。イベント購読は初回1度きりで張るため、クロージャが
  // 初期値に固定されないよう runRef と同じく ref 経由で最新値を読む。
  const hasTargetRef = useRef(false);
  useEffect(() => {
    hasTargetRef.current = targetIds.length > 0;
  }, [targetIds]);

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
      minQ: number,
      minVal: number,
    ) => {
      const gen = ++searchGenRef.current;
      const isCurrent = () => gen === searchGenRef.current;
      inflightRef.current += 1;
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
          minQuality: minQ,
          minModuleValue: minVal,
        });
        if (!isCurrent()) return; // 片付け済み or より新しい探索がある
        setElapsedMs(performance.now() - startedAt);
        setResult(res);
        setAppliedRankMode(mode);
        if (gen > staleSinceGenRef.current) setModulesStale(false);
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
        if (!isCurrent()) return;
        setError(String(e));
        setResult(null);
        setElapsedMs(null);
      } finally {
        // 結果を捨てた探索も含め、全部終わってから実行中表示を消す。
        inflightRef.current -= 1;
        if (inflightRef.current === 0) setLoading(false);
      }
    },
    [t],
  );

  const runOptimize = useCallback(
    () =>
      runWith(
        selection,
        requireLevels,
        category,
        topK,
        slotCount,
        hardExclude,
        rankMode,
        minQuality,
        minModuleValue,
      ),
    [
      runWith,
      selection,
      requireLevels,
      category,
      topK,
      slotCount,
      hardExclude,
      rankMode,
      minQuality,
      minModuleValue,
    ],
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
    const unModules = listen<number>("modules-updated", (ev) => {
      refreshStatus();
      // 0 件（属性値が読めないまま初検知のキャラへ切り替わった等）では探索できない。
      // 走らせると「条件を緩めて」という的外れなエラーになるので、別キャラのデータで
      // 出した古い結果を片付けるだけにする（理由はバナーと空状態が説明する）。
      if (ev.payload === 0) {
        discardResults();
        setModulesStale(false);
        return;
      }
      // 目標属性が未選択のときは自動再探索しない。目標なしの探索は B&B の上界が緩く
      // 最悪ケースになり（所持458件の実測で目標あり4.5s に対し目標なし41.6s）、装備を
      // 変えるたびに数十秒 CPU を占有してしまう。代わりに「結果が古い」ことを示して
      // 手動実行に委ねる（select_character もキャラ切替の都度これを emit するため、
      // 抑止しないとキャラを切り替えるだけで最悪ケースが走る）。
      if (!hasTargetRef.current) {
        markModulesStale();
        return;
      }
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
  }, [refreshStatus, discardResults, markModulesStale]);

  // ユーザーの明示選択。保存値の上書きはここだけで行う（復元側は保存値を書き換えない）。
  // status?.selected_char_id は自動解決の結果に過ぎず「既に固定済み」を意味しない
  // ため、一致していても早期リターンせず必ず select_character を呼んで明示的に固定する
  // （さもないと「自動採用中のキャラを明示的に選び直す」操作が永続化されない）。
  const onSelectCharacter = async (charId: string) => {
    if (!charId) return;
    restoredSelectionRef.current = true;
    // 他キャラのモジュールで出した解を残さない（切替前に始めた探索の結果も捨てる）。
    discardResults();
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
    runWith(
      next,
      requireLevels,
      category,
      topK,
      slotCount,
      hardExclude,
      rankMode,
      minQuality,
      minModuleValue,
    );
  };

  const resetCategory = () => {
    setCategory("all");
    runWith(
      selection,
      requireLevels,
      "all",
      topK,
      slotCount,
      hardExclude,
      rankMode,
      minQuality,
      minModuleValue,
    );
  };

  // 入力欄以外の経路（プリセット適用）から表示件数を変える時の唯一の口。確定値と
  // 入力欄の表示を必ず同時に更新する（片方だけ更新すると入力欄に古い値が残る）。
  // 正規化後の値を返すので、呼び出し側は探索へ渡す値をここから受け取れる。
  const applyTopK = (v: number): number => {
    const k = normalizeTopK(v);
    setTopK(k);
    setTopKDraft(String(k));
    return k;
  };

  // 入力中は数字のみを受け取り、範囲内になった時点で確定値へ反映する
  // （localStorage への記憶もこの反映経路で走る）。空欄・範囲外は確定させず、
  // commitTopKDraft（blur / Enter）で丸めて確定する。
  const onTopKInput = (raw: string) => {
    const digits = raw.replace(/\D/g, "");
    setTopKDraft(digits);
    const n = Number.parseInt(digits, 10);
    if (Number.isInteger(n) && n >= TOP_K_MIN && n <= TOP_K_MAX) setTopK(n);
  };

  // 入力欄を離れた時点で表示を確定値へ揃える。空欄・数値でない入力は直前の確定値へ
  // 戻し、0 や上限超えは範囲内へ丸める。
  const commitTopKDraft = () => {
    const n = Number.parseInt(topKDraft, 10);
    applyTopK(Number.isNaN(n) ? topK : n);
  };

  // 切り捨て設定を確定する（確認済み、または確認が不要な変更）。入力欄の表示も揃える。
  const applyCutoff = (quality: number, value: number) => {
    setMinQuality(quality);
    setMinModuleValue(value);
    setMinValueDraft(String(value));
  };

  // 切り捨て設定の変更要求。「絞り込みなし」から絞り込みを有効にするときだけ確認を挟む
  // （有効なままの値の調整や、解除の操作では出さない）。
  const requestCutoff = (quality: number, value: number) => {
    const wasOff = minQuality === 0 && minModuleValue === 0;
    const willBeOn = quality > 0 || value > 0;
    // 金品質(CUTOFF_MAX_QUALITY)は実測で最も1位が壊れやすいため、既に絞り込み中でも
    // 新たに選ぶときは必ず確認する（紫→金の変更が無警告で通ると、最も危険な設定へ
    // 黙って移ってしまう）。
    const newlyMaxQuality =
      quality >= CUTOFF_MAX_QUALITY && minQuality < CUTOFF_MAX_QUALITY;
    if ((wasOff && willBeOn) || newlyMaxQuality) {
      setPendingCutoff({ quality, value });
      return;
    }
    applyCutoff(quality, value);
  };

  const onMinValueInput = (raw: string) => setMinValueDraft(raw.replace(/\D/g, ""));

  // 入力欄を離れた時点で 0〜CUTOFF_MAX_VALUE に丸めて確定する。空欄・数値でない入力は
  // 直前の確定値へ戻す。確認ダイアログが要るケースは requestCutoff が判断する。
  const commitMinValue = () => {
    const n = Number.parseInt(minValueDraft, 10);
    const v = Number.isNaN(n) ? minModuleValue : Math.min(Math.max(n, 0), CUTOFF_MAX_VALUE);
    if (v === minModuleValue) {
      setMinValueDraft(String(minModuleValue));
      return;
    }
    setMinValueDraft(String(v));
    requestCutoff(minQuality, v);
  };

  // 確認ダイアログのキャンセル時は、先行して書き換えていた入力欄の表示も確定値へ戻す。
  const cancelCutoff = () => {
    setPendingCutoff(null);
    setMinValueDraft(String(minModuleValue));
  };

  const confirmCutoff = () => {
    if (pendingCutoff) applyCutoff(pendingCutoff.quality, pendingCutoff.value);
    setPendingCutoff(null);
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
    // 旧プリセットの topK は当時の選択肢（3/5/10）や上限変更前の値のまま入っている。
    // 表示・探索の両方で同じ正規化値を使う。
    const k = applyTopK(p.topK);
    // 旧プリセット（切り捨て追加前）には無いため 0（絞り込みなし）へ正規化する。
    // プリセット適用は本人が保存した設定の再現なので、確認ダイアログは出さない。
    const minQ = normalizeCutoff(p.minQuality, CUTOFF_MAX_QUALITY);
    const minVal = normalizeCutoff(p.minModuleValue, CUTOFF_MAX_VALUE);
    setSelection(p.selection);
    setRequireLevels(p.requireLevels);
    setCategory(p.category);
    setSlotCount(slots);
    setHardExclude(hardExcl);
    setRankMode(mode);
    setMinQuality(minQ);
    setMinModuleValue(minVal);
    setMinValueDraft(String(minVal));
    setTab("results");
    runWith(p.selection, p.requireLevels, p.category, k, slots, hardExcl, mode, minQ, minVal);
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
      minQuality,
      minModuleValue,
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
  // 候補の切り捨てが有効か。有効な間は「所持全体の最適解ではないかもしれない」ことを
  // 常時表示する（確認ダイアログは有効化の1回きりなので、それだけでは足りない）。
  const cutoffActive = minQuality > 0 || minModuleValue > 0;

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
            <input
              type="number"
              inputMode="numeric"
              min={TOP_K_MIN}
              max={TOP_K_MAX}
              step={1}
              value={topKDraft}
              onChange={(e) => onTopKInput(e.currentTarget.value)}
              onBlur={commitTopKDraft}
              onKeyDown={(e) => {
                if (e.key === "Enter") e.currentTarget.blur();
              }}
              aria-label={t("section.topK")}
              className="h-8 w-full rounded-lg border border-slate-700 bg-slate-800/60 px-2.5 text-xs font-medium text-slate-100 outline-none transition focus:border-indigo-500"
            />
            <p className="mt-1.5 text-xs leading-relaxed text-slate-400">
              {t("topK.hint", { min: TOP_K_MIN, max: TOP_K_MAX })}
            </p>
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

          <section>
            <h2 className="mb-2 text-xs font-bold uppercase tracking-wider text-slate-300">
              {t("section.cutoff")}
            </h2>
            <div className="flex gap-1 rounded-lg bg-slate-800/60 p-1">
              {MIN_QUALITY_OPTIONS.map((q) => (
                <button
                  key={q}
                  onClick={() => requestCutoff(q, minModuleValue)}
                  className={`flex h-8 flex-1 items-center justify-center rounded-md text-xs font-medium transition ${
                    minQuality === q
                      ? "bg-indigo-500 text-white shadow"
                      : "text-slate-300 hover:bg-slate-700/60"
                  }`}
                >
                  {q === 0 ? t("cutoff.qualityAll") : t(`cutoff.quality${q}`)}
                </button>
              ))}
            </div>
            <div className="mt-1.5 flex items-center gap-2">
              <span className="shrink-0 text-xs text-slate-400">{t("cutoff.minValue")}</span>
              <input
                type="number"
                inputMode="numeric"
                min={0}
                max={CUTOFF_MAX_VALUE}
                step={1}
                value={minValueDraft}
                onChange={(e) => onMinValueInput(e.currentTarget.value)}
                onBlur={commitMinValue}
                onKeyDown={(e) => {
                  if (e.key === "Enter") e.currentTarget.blur();
                }}
                aria-label={t("cutoff.minValue")}
                className="h-8 w-full min-w-0 rounded-lg border border-slate-700 bg-slate-800/60 px-2.5 text-xs font-medium text-slate-100 outline-none transition focus:border-indigo-500"
              />
            </div>
            <p
              className={`mt-1.5 text-xs leading-relaxed ${
                cutoffActive ? "text-amber-400" : "text-slate-400"
              }`}
            >
              {cutoffActive
                ? t("cutoff.activeHint")
                : t("cutoff.hint", { max: CUTOFF_MAX_VALUE })}
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
          {status?.value_issue && (
            <ValueIssueBanner issue={status.value_issue} moduleCount={status.module_count} />
          )}

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

              {/* 目標属性が未選択で自動再探索をスキップした場合の告知。エラー(amber)と
                  区別できるよう情報色(sky)にする。 */}
              {modulesStale && (
                <div className="mb-4 rounded-lg border border-sky-700/50 bg-sky-500/10 px-4 py-2.5 text-sm text-sky-200">
                  {t("results.modulesStale")}
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
                    {cutoffActive && (
                      <span className="text-amber-400"> ・{t("results.cutoffActive")}</span>
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
                        {/* 値の取得異常で取れていないときに「マップ移動で取得」と案内すると、
                            同じ失敗を繰り返させてしまう。 */}
                        <p className="mt-1 text-xs">
                          {status?.value_issue?.kind === "missing"
                            ? t("empty.noModulesDescValueIssue")
                            : t("empty.noModulesDesc")}
                        </p>
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

      {pendingCutoff && (
        <CutoffWarningDialog
          quality={pendingCutoff.quality}
          value={pendingCutoff.value}
          onConfirm={confirmCutoff}
          onCancel={cancelCutoff}
        />
      )}
    </div>
  );
}
