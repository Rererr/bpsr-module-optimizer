// Rust DTO に対応する型定義。

// 属性の選択状態。AttributePicker / App / プリセットで共有するドメイン型。
export type AttrState = "none" | "target" | "exclude";

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
}

export interface Solution {
  modules: Module[];
  link_effect: number; // リンク効果（全属性値の合計）= 表示スコア
  lv6_count: number;
  lv5_count: number;
  selected_lv6: number;
  level_sum: number;
  breakdown: AttrBreakdown[];
}

export interface OptimizeResult {
  solutions: Solution[];
  candidate_count: number;
  combinations: number;
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
