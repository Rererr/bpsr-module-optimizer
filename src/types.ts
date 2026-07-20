// Rust DTO に対応する型定義。

// 属性の選択状態。AttributePicker / App / プリセットで共有するドメイン型。
export type AttrState = "none" | "target" | "exclude";

// ランキング順序モード。Rust 側 optimizer::RankMode の serde snake_case 表現と一致させること
// （optimizer.rs の rank_mode_serde_snake_case テストで固定済み）。
// - "link"（既定）: Lv6数の次に評価リンク（合計値）を優先し、Lv5数はその後に回す。
// - "lv5": Lv6数の次にLv5到達数（個数）を優先し、評価リンクはその後に回す。
// 2つの順序は一般には互いに他方の1位を再現できない（データ依存で必要な保持件数が
// 数百〜数千に達しうると実測確認済み）ため、モードを切り替えたら再検索が必要。
export type RankMode = "link" | "lv5";

export interface AttrMeta {
  id: number;
  name: string;
  special: boolean;
  group: string;
}

export interface Part {
  attr_id: number;
  attr_name: string;
  value: number;
}

export interface Module {
  key: number;
  uuid: number;
  config_id: number;
  name: string;
  category: string; // "attack" | "guardian" | "support" | "unknown"
  quality: number;
  parts: Part[];
}

export interface AttrBreakdown {
  attr_id: number;
  attr_name: string;
  value: number;
  level: number; // 0〜6
  selected: boolean;
  // ソフト除外指定された属性か。true の場合、lv6_count 等の集計には含まれない。
  soft_excluded: boolean;
}

export interface Solution {
  modules: Module[];
  link_effect: number; // リンク効果（全属性値の合計・表示用の真値）。ソフト除外属性の値も含む
  eval_link: number; // 評価スコア（ソフト除外を除いた counted 属性値の合計）。ランキングの実体
  lv6_count: number;
  lv5_count: number;
  selected_lv6: number;
  selected_present: number; // 選択属性のうち結果に存在する数（Lv1以上）。ランキング最優先キー
  breakdown: AttrBreakdown[];
}

export interface OptimizeResult {
  solutions: Solution[];
  candidate_count: number;
  combinations: number;
  // 実際に使われた探索エンジン。"cpu" | "gpu"。通常ビルドは常に "cpu"。
  engine: string;
}

export interface StatusDto {
  capture_state: "init" | "running" | "failed";
  module_count: number;
  last_update_ms: number | null;
  source: "capture" | "dump" | "none";
  last_game_packet_ms_ago: number | null;
}

// 属性レベルの閾値境界（属性値 → Lv0〜6）。UI のレベル表示に使う。
export const ATTR_THRESHOLDS = [1, 4, 8, 12, 16, 20];

// 検索条件のプリセット（localStorage に保存）。
export interface SearchPreset {
  id: string;
  name: string;
  selection: Record<number, AttrState>;
  requireLevels: Record<number, number>;
  category: string;
  topK: number;
  // 装備枠数（4 または 5）。旧プリセットには存在しないため適用側で 4 にフォールバックする。
  slotCount: number;
  // true ならハード除外（該当モジュールを候補から丸ごと除外）、false ならソフト除外
  // （属性のみランキング集計から除外）。旧プリセットには存在しないため false（ソフト）に
  // フォールバックする。
  hardExclude: boolean;
  // ランキング順序モード。旧プリセット（本機能追加前）には存在しないため、適用側で
  // "link"（既定・従来の唯一の順序）にフォールバックする。
  rankMode: RankMode;
  createdAt: number;
}

// お気に入りに保存したビルド構成（解候補のスナップショット）。
export interface SavedBuild {
  id: string; // buildIdOf(solution): 各モジュール uuid を昇順連結
  name: string; // 既定は自動命名、ユーザー編集可
  solution: Solution; // 保存時点の値（モジュール集合が変わっても保持）
  targetIds: number[]; // 保存時の目標属性。カード内の該当パーツ強調を再現
  savedAt: number;
}
