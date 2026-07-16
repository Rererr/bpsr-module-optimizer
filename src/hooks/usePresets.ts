import { useCallback, useEffect, useState } from "react";
import type { SearchPreset } from "../types";
import { STORAGE_KEYS, loadJSON, saveJSON } from "../storage";

// プリセット保存に必要な検索条件（id/name/createdAt を除いた本体）。
export type PresetConfig = Pick<
  SearchPreset,
  "selection" | "requireLevels" | "category" | "topK" | "slotCount" | "hardExclude"
>;

function newId(): string {
  return `${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`;
}

export interface UsePresets {
  presets: SearchPreset[];
  save: (name: string, config: PresetConfig) => void;
  rename: (id: string, name: string) => void;
  remove: (id: string) => void;
}

/** 検索条件プリセットの一覧と CRUD。localStorage に永続化する。 */
export function usePresets(): UsePresets {
  const [presets, setPresets] = useState<SearchPreset[]>(() =>
    loadJSON<SearchPreset[]>(STORAGE_KEYS.presets, []),
  );

  useEffect(() => {
    saveJSON(STORAGE_KEYS.presets, presets);
  }, [presets]);

  const save = useCallback((name: string, config: PresetConfig) => {
    const trimmed = name.trim();
    if (!trimmed) return;
    setPresets((prev) => [
      ...prev,
      { id: newId(), name: trimmed, createdAt: Date.now(), ...config },
    ]);
  }, []);

  const rename = useCallback((id: string, name: string) => {
    const trimmed = name.trim();
    if (!trimmed) return;
    setPresets((prev) =>
      prev.map((p) => (p.id === id ? { ...p, name: trimmed } : p)),
    );
  }, []);

  const remove = useCallback((id: string) => {
    setPresets((prev) => prev.filter((p) => p.id !== id));
  }, []);

  return { presets, save, rename, remove };
}
