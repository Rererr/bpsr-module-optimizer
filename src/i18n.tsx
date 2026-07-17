// 軽量 i18n。日本語を原文、英語を対訳とする。言語はヘッダのトグルで手動切替し、
// localStorage に永続化する（OS ロケール自動判定はしない＝切替は1方式）。
// 属性名・モジュール種別名は Rust から日本語で届くため、英語時のみ id 起点で上書きする
// （DTO は attr_id / config_id を持つので再フェッチ不要でライブ切替できる）。

import {
  createContext,
  useContext,
  useEffect,
  useMemo,
  useState,
  type ReactNode,
} from "react";
import { STORAGE_KEYS, loadJSON, saveJSON } from "./storage";

export type Lang = "ja" | "en";

type Vars = Record<string, string | number>;

// UI 文言。キーは <namespace>.<name>。{var} はプレースホルダ。
const DICT: Record<Lang, Record<string, string>> = {
  ja: {
    "app.title": "BPSR モジュール最適化",
    "app.subtitle": "Lv6を優先しリンク効果が最大になる{n}枠を探索",
    "status.running": "キャプチャ稼働中",
    "status.failed": "キャプチャ失敗(要管理者権限)",
    "status.waiting": "キャプチャ待機",
    "status.modulesUnit": "件",
    "status.reload": "保存データから読込",
    "status.reloadTitle": "保存した owned_modules.json を読み込む",
    "time.never": "未取得",
    "time.now": "たった今",
    "time.secAgo": "{n}秒前",
    "time.minAgo": "{n}分前",
    "time.hourAgo": "{n}時間前",
    "lang.switch": "言語を切り替え",
    "section.presets": "プリセット",
    "section.attributes": "属性を選択",
    "section.category": "カテゴリ",
    "section.slotCount": "スロット数",
    "section.minLv": "属性ごとの下限Lv",
    "section.topK": "表示件数",
    "run": "最適化を実行",
    "topK.option": "上位{k}",
    "slotCount.option": "{n}枠",
    "tab.results": "結果",
    "tab.favorites": "お気に入り",
    "results.summary":
      "上位 {sets} セット / 候補 {candidates} 件から {combos} 通りを探索",
    "results.elapsed": "処理時間 {t}",
    "empty.noModulesTitle": "所持モジュール未取得",
    "empty.noModulesDesc":
      "管理者権限でゲームのマップ移動で取得してください（取得後は自動保存され、次回起動時に復元されます）",
    "empty.readyTitle": "目標属性を選んで「最適化を実行」を押してください",
    "empty.readyDesc": "所持 {n} 件から最良の{slots}枠を探索します",
    "error.tooFewCandidates":
      "候補モジュールが {c} 件で{slots}枠に満たません（条件を緩めてください）",
    "error.noReqMatch":
      "指定した下限Lvをすべて満たす組み合わせがありません（下限を下げるか属性を減らしてください）",
    "error.noMatch": "条件に合う組み合わせがありません",
    "picker.target": "目標",
    "picker.exclude": "除外",
    "picker.targetN": "目標 {n}",
    "picker.excludeN": "除外 {n}",
    "picker.clear": "クリア",
    "picker.hardExclude": "除外を完全排除する（ハード除外）",
    "req.empty": "目標属性（緑）を選ぶと、ここで属性ごとに下限Lvを設定できます。",
    "req.noMin": "下限なし",
    "req.minLv": "Lv{n}以上",
    "cond.label": "条件",
    "cond.removeTarget": "目標「{name}」を解除",
    "cond.removeExclude": "除外「{name}」を解除",
    "cond.removeCategory": "カテゴリ条件を解除",
    "common.remove": "解除",
    "common.save": "保存",
    "common.cancel": "キャンセル",
    "common.close": "閉じる",
    "card.linkEffect": "リンク効果",
    "card.evalLink": "評価スコア",
    "card.softExcluded": "ソフト除外",
    "card.selectedLv6": "選択Lv6 ×{n}",
    "card.targetsPartial": "目標 {n}/{total} 含む",
    "card.targetsMissing": "含められなかった目標: {names}",
    "card.targetsMissingGeneric": "一部の目標属性を含められませんでした",
    "card.favRemove": "お気に入りから削除",
    "card.favAdd": "お気に入りに追加",
    "fav.emptyTitle": "お気に入りのビルドはまだありません",
    "fav.emptyDesc": "結果カードの ★ を押すと、ここに構成を保存できます",
    "fav.compareN": "{n}件を比較",
    "fav.clearSel": "選択解除",
    "fav.compareHint": "カードの「比較」で2〜3件を選択",
    "fav.compareOneMore": "あと1件選ぶと比較できます",
    "fav.compareLabel": "比較",
    "fav.compareMax": "比較は最大3件まで",
    "fav.compareAdd": "比較に追加",
    "fav.addToCompare": "「{name}」を比較に追加",
    "fav.maxSuffix": "（最大3件に達しています）",
    "fav.saveName": "名前を保存",
    "fav.cancelEdit": "名前の編集をキャンセル",
    "fav.renameName": "「{name}」の名前を編集",
    "fav.rename": "名前を編集",
    "fav.deleteName": "「{name}」を削除",
    "preset.empty": "条件を保存しておくと、ワンクリックで呼び出せます。",
    "preset.apply": "「{name}」を適用",
    "preset.targetN": "目標{n}",
    "preset.excludeN": "除外{n}",
    "preset.deleteName": "プリセット「{name}」を削除",
    "preset.namePlaceholder": "プリセット名",
    "preset.save": "プリセットを保存",
    "preset.cancelSave": "保存をキャンセル",
    "preset.saveCurrent": "現在の条件を保存",
    "preset.noneToSave": "保存できる条件がありません",
    "confirm.again": "{label}（もう一度押して確定）",
    "confirm.againTitle": "もう一度押して削除",
    "compare.title": "ビルド比較（{n}件）",
    "compare.lv6": "Lv6数",
    "compare.lv5": "Lv5数",
    "footer.contact": "お問い合わせ",
    "footer.reportGithub": "GitHubで報告",
    "footer.show": "フッターを表示",
    "footer.hide": "フッターを隠す",
  },
  en: {
    "app.title": "BPSR Module Optimizer",
    "app.subtitle": "Finds the {n} slots that prioritize Lv6 and maximize link effect",
    "status.running": "Capturing",
    "status.failed": "Capture failed (admin required)",
    "status.waiting": "Waiting for capture",
    "status.modulesUnit": "modules",
    "status.reload": "Load saved data",
    "status.reloadTitle": "Load the saved owned_modules.json",
    "time.never": "never",
    "time.now": "just now",
    "time.secAgo": "{n}s ago",
    "time.minAgo": "{n}m ago",
    "time.hourAgo": "{n}h ago",
    "lang.switch": "Switch language",
    "section.presets": "Presets",
    "section.attributes": "Select Attributes",
    "section.category": "Category",
    "section.slotCount": "Slots",
    "section.minLv": "Min Lv per Attribute",
    "section.topK": "Result Count",
    "run": "Run Optimization",
    "topK.option": "Top {k}",
    "slotCount.option": "{n} slots",
    "tab.results": "Results",
    "tab.favorites": "Favorites",
    "results.summary":
      "Top {sets} sets / searched {combos} combinations from {candidates} candidates",
    "results.elapsed": "Time {t}",
    "empty.noModulesTitle": "No modules captured yet",
    "empty.noModulesDesc":
      "Launch as administrator and change maps in-game to capture them (captured data is saved automatically and restored on the next launch).",
    "empty.readyTitle": "Pick target attributes and press “Run Optimization”",
    "empty.readyDesc": "Searches the best {slots} slots from your {n} modules",
    "error.tooFewCandidates":
      "Only {c} candidate modules — not enough for {slots} slots (relax the conditions).",
    "error.noReqMatch":
      "No combination satisfies all the specified minimum levels (lower them or reduce attributes).",
    "error.noMatch": "No combination matches the conditions.",
    "picker.target": "Target",
    "picker.exclude": "Exclude",
    "picker.targetN": "Target {n}",
    "picker.excludeN": "Exclude {n}",
    "picker.clear": "Clear",
    "picker.hardExclude": "Fully exclude (hard exclude)",
    "req.empty": "Pick target attributes (green) to set a minimum Lv for each here.",
    "req.noMin": "No min",
    "req.minLv": "Lv{n}+",
    "cond.label": "Conditions",
    "cond.removeTarget": "Remove target “{name}”",
    "cond.removeExclude": "Remove exclude “{name}”",
    "cond.removeCategory": "Remove category filter",
    "common.remove": "Remove",
    "common.save": "Save",
    "common.cancel": "Cancel",
    "common.close": "Close",
    "card.linkEffect": "Link Effect",
    "card.evalLink": "Eval Score",
    "card.softExcluded": "Soft-excluded",
    "card.selectedLv6": "Target Lv6 ×{n}",
    "card.targetsPartial": "{n}/{total} targets included",
    "card.targetsMissing": "Couldn't include: {names}",
    "card.targetsMissingGeneric": "Some target attributes couldn't be included",
    "card.favRemove": "Remove from favorites",
    "card.favAdd": "Add to favorites",
    "fav.emptyTitle": "No favorite builds yet",
    "fav.emptyDesc": "Press the ★ on a result card to save a build here.",
    "fav.compareN": "Compare {n}",
    "fav.clearSel": "Clear selection",
    "fav.compareHint": "Select 2–3 with the “Compare” checkbox",
    "fav.compareOneMore": "Select one more to compare",
    "fav.compareLabel": "Compare",
    "fav.compareMax": "Up to 3 builds",
    "fav.compareAdd": "Add to comparison",
    "fav.addToCompare": "Add “{name}” to comparison",
    "fav.maxSuffix": " (max 3 reached)",
    "fav.saveName": "Save name",
    "fav.cancelEdit": "Cancel editing",
    "fav.renameName": "Rename “{name}”",
    "fav.rename": "Rename",
    "fav.deleteName": "Delete “{name}”",
    "preset.empty": "Save conditions to recall them with one click.",
    "preset.apply": "Apply “{name}”",
    "preset.targetN": "Target {n}",
    "preset.excludeN": "Exclude {n}",
    "preset.deleteName": "Delete preset “{name}”",
    "preset.namePlaceholder": "Preset name",
    "preset.save": "Save preset",
    "preset.cancelSave": "Cancel saving",
    "preset.saveCurrent": "Save current conditions",
    "preset.noneToSave": "No conditions to save",
    "confirm.again": "{label} (press again to confirm)",
    "confirm.againTitle": "Press again to delete",
    "compare.title": "Build comparison ({n})",
    "compare.lv6": "Lv6 count",
    "compare.lv5": "Lv5 count",
    "footer.contact": "Contact",
    "footer.reportGithub": "Report on GitHub",
    "footer.show": "Show footer",
    "footer.hide": "Hide footer",
  },
};

// 英語の属性名（attr_id → 名前）。ゲーム公式英語名（BPSR-ZDPS ModEffectTable.EffectName 由来, MIT）。
const ATTR_EN: Record<number, string> = {
  1110: "Strength Boost",
  1111: "Agility Boost",
  1112: "Intellect Boost",
  1113: "Special Attack",
  1114: "Elite Strike",
  1205: "Healing Boost",
  1206: "Healing Enhance",
  1307: "Resistance",
  1308: "Armor",
  1407: "Cast Focus",
  1408: "Attack Speed",
  1409: "Crit Focus",
  1410: "Luck Focus",
  2104: "DMG Stack",
  2105: "Agile",
  2204: "Life Condense",
  2205: "First Aid",
  2304: "Final Protection",
  2404: "Life Wave",
  2405: "Life Steal",
  2406: "Team Luck & Crit",
};

// 英語のモジュール種別名（config_id → 名前）。
const MODULE_EN: Record<number, string> = {
  5500101: "Basic Attack",
  5500102: "Advanced Attack",
  5500103: "Superior Attack",
  5500104: "EXC Attack (Refined)",
  5500201: "Basic Support",
  5500202: "Advanced Support",
  5500203: "Superior Support",
  5500204: "EXC Support (Refined)",
  5500301: "Basic Guard",
  5500302: "Advanced Guard",
  5500303: "Superior Guard",
  5500304: "EXC Guard (Refined)",
};

const CATEGORY: Record<Lang, Record<string, string>> = {
  ja: { all: "すべて", attack: "攻撃", guardian: "防御", support: "支援", unknown: "不明" },
  en: { all: "All", attack: "Attack", guardian: "Guard", support: "Support", unknown: "Unknown" },
};

const GROUP: Record<Lang, Record<string, string>> = {
  ja: {
    Basic: "基礎ステータス",
    Combat: "攻撃",
    Focus: "集中",
    Resist: "耐性",
    Support: "回復/支援",
    Ultra: "上位(Ultra)",
  },
  en: {
    Basic: "Basics",
    Combat: "Combat",
    Focus: "Focus",
    Resist: "Resist",
    Support: "Heal / Support",
    Ultra: "Ultra",
  },
};

interface I18nApi {
  lang: Lang;
  setLang: (l: Lang) => void;
  t: (key: string, vars?: Vars) => string;
  /** 英語時のみ attr_id 起点で上書き、日本語時はサーバ提供名を返す。 */
  attrName: (id: number, fallback: string) => string;
  moduleName: (configId: number, fallback: string) => string;
  categoryLabel: (cat: string) => string;
  groupLabel: (group: string) => string;
}

const Ctx = createContext<I18nApi | null>(null);

function interpolate(s: string, vars?: Vars): string {
  if (!vars) return s;
  return s.replace(/\{(\w+)\}/g, (_, k) => (k in vars ? String(vars[k]) : `{${k}}`));
}

export function I18nProvider({ children }: { children: ReactNode }) {
  const [lang, setLang] = useState<Lang>(() => loadJSON<Lang>(STORAGE_KEYS.lang, "ja"));

  useEffect(() => {
    saveJSON(STORAGE_KEYS.lang, lang);
    document.documentElement.lang = lang;
  }, [lang]);

  const value = useMemo<I18nApi>(
    () => ({
      lang,
      setLang,
      t: (key, vars) => interpolate(DICT[lang][key] ?? DICT.ja[key] ?? key, vars),
      attrName: (id, fallback) => (lang === "en" ? ATTR_EN[id] ?? fallback : fallback),
      moduleName: (configId, fallback) =>
        lang === "en" ? MODULE_EN[configId] ?? fallback : fallback,
      categoryLabel: (cat) => CATEGORY[lang][cat] ?? cat,
      groupLabel: (group) => GROUP[lang][group] ?? group,
    }),
    [lang],
  );

  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

export function useI18n(): I18nApi {
  const v = useContext(Ctx);
  if (!v) throw new Error("useI18n は I18nProvider 内で使用してください");
  return v;
}
