// Tauri コマンドの薄いラッパ。

import { invoke } from "@tauri-apps/api/core";
import type { AttrMeta, Module, OptimizeResult, StatusDto } from "./types";

export function getModules(): Promise<Module[]> {
  return invoke<Module[]>("get_modules");
}

export function getAttributes(): Promise<AttrMeta[]> {
  return invoke<AttrMeta[]>("get_attributes");
}

export function captureStatus(): Promise<StatusDto> {
  return invoke<StatusDto>("capture_status");
}

export interface OptimizeArgs {
  selectedIds: number[];
  category: string | null;
  excludeIds: number[];
  /// 属性ごとの下限レベル [attr_id, min_level]。
  requirements: [number, number][];
  topK: number;
}

export function optimize(args: OptimizeArgs): Promise<OptimizeResult> {
  return invoke<OptimizeResult>("optimize", {
    selectedIds: args.selectedIds,
    category: args.category,
    excludeIds: args.excludeIds,
    requirements: args.requirements,
    topK: args.topK,
  });
}

export function reloadFromDump(path?: string): Promise<number> {
  return invoke<number>("reload_from_dump", { path: path ?? null });
}
