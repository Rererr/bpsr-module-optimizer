import { useCallback, useEffect, useMemo, useState } from "react";
import type { SavedBuild, Solution } from "../types";
import { STORAGE_KEYS, loadJSON, saveJSON } from "../storage";

/** ビルド構成の同一性キー。モジュール uuid を昇順連結（枠数非依存・順序非依存）。 */
export function buildIdOf(solution: Solution): string {
  return solution.modules
    .map((m) => m.uuid)
    .sort((a, b) => a - b)
    .join("-");
}

// 既定のビルド名（保存時刻ベース）。後からユーザーが変更できる。
function defaultName(): string {
  const d = new Date();
  const p = (n: number) => String(n).padStart(2, "0");
  return `ビルド ${p(d.getMonth() + 1)}/${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
}

export interface UseFavorites {
  favorites: SavedBuild[];
  isFavorite: (solution: Solution) => boolean;
  /** 未保存なら追加、保存済みなら削除（トグル）。 */
  toggle: (solution: Solution) => void;
  rename: (id: string, name: string) => void;
  remove: (id: string) => void;
}

/** お気に入りビルドの一覧と CRUD。localStorage に永続化する。 */
export function useFavorites(): UseFavorites {
  const [favorites, setFavorites] = useState<SavedBuild[]>(() =>
    loadJSON<SavedBuild[]>(STORAGE_KEYS.favorites, []),
  );

  useEffect(() => {
    saveJSON(STORAGE_KEYS.favorites, favorites);
  }, [favorites]);

  const ids = useMemo(() => new Set(favorites.map((f) => f.id)), [favorites]);

  const isFavorite = useCallback(
    (solution: Solution) => ids.has(buildIdOf(solution)),
    [ids],
  );

  const toggle = useCallback((solution: Solution) => {
    const id = buildIdOf(solution);
    // 目標属性は解（存在した目標＝breakdown.selected かつ level>=1）から復元し、スナップショットと
    // 整合させる。level ガードにより保存 targetIds 数 === selected_present が構造的に保証される。
    const targetIds = solution.breakdown
      .filter((b) => b.selected && b.level >= 1)
      .map((b) => b.attr_id);
    setFavorites((prev) => {
      if (prev.some((f) => f.id === id)) return prev.filter((f) => f.id !== id);
      return [
        ...prev,
        { id, name: defaultName(), solution, targetIds, savedAt: Date.now() },
      ];
    });
  }, []);

  const rename = useCallback((id: string, name: string) => {
    const trimmed = name.trim();
    if (!trimmed) return;
    setFavorites((prev) =>
      prev.map((f) => (f.id === id ? { ...f, name: trimmed } : f)),
    );
  }, []);

  const remove = useCallback((id: string) => {
    setFavorites((prev) => prev.filter((f) => f.id !== id));
  }, []);

  return { favorites, isFavorite, toggle, rename, remove };
}
