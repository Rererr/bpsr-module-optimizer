// Rust DTO に対応する型定義。

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

export const CATEGORY_LABELS: Record<string, string> = {
  all: "すべて",
  attack: "攻撃",
  guardian: "防御",
  support: "支援",
  unknown: "不明",
};

// 属性レベルの閾値境界（属性値 → Lv0〜6）。UI のレベル表示に使う。
export const ATTR_THRESHOLDS = [1, 4, 8, 12, 16, 20];
