// Tauri コマンドの薄いラッパ。

import { invoke } from "@tauri-apps/api/core";
import type { AttrMeta, Module, OptimizeResult, RankMode, StatusDto } from "./types";

export function getModules(): Promise<Module[]> {
  return invoke<Module[]>("get_modules");
}

export function getAttributes(): Promise<AttrMeta[]> {
  return invoke<AttrMeta[]>("get_attributes");
}

export function captureStatus(): Promise<StatusDto> {
  return invoke<StatusDto>("capture_status");
}

/** 表示/探索対象のキャラを切り替える。戻り値は切替後のモジュール件数。 */
export function selectCharacter(charId: string): Promise<number> {
  return invoke<number>("select_character", { charId });
}

export interface OptimizeArgs {
  selectedIds: number[];
  category: string | null;
  /// ハード除外: いずれかを含むモジュールを候補から丸ごと除外。
  excludeIds: number[];
  /// ソフト除外: モジュールは候補に残すが、該当属性はランキング集計から除外する。
  softExcludeIds: number[];
  /// 属性ごとの下限レベル [attr_id, min_level]。
  requirements: [number, number][];
  topK: number;
  /// 装備枠数（4 または 5）。
  slotCount: number;
  /// ランキング順序モード。
  rankMode: RankMode;
  /// 候補の切り捨て: 品質がこれ未満のモジュールを候補から外す（0=無効）。
  minQuality: number;
  /// 候補の切り捨て: 属性値の合計がこれ未満のモジュールを候補から外す（0=無効）。
  minModuleValue: number;
}

export function optimize(args: OptimizeArgs): Promise<OptimizeResult> {
  return invoke<OptimizeResult>("optimize", {
    selectedIds: args.selectedIds,
    category: args.category,
    excludeIds: args.excludeIds,
    softExcludeIds: args.softExcludeIds,
    requirements: args.requirements,
    topK: args.topK,
    slotCount: args.slotCount,
    rankMode: args.rankMode,
    minQuality: args.minQuality,
    minModuleValue: args.minModuleValue,
  });
}

export function reloadFromDump(path?: string): Promise<number> {
  return invoke<number>("reload_from_dump", { path: path ?? null });
}
