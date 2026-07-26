import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import type { SavedBuild, Solution } from "../types";
import { STORAGE_KEYS, hasKey, loadJSON, saveJSON } from "../storage";
import { useI18n } from "../i18n";

/** ビルド構成の同一性キー。モジュール uuid を昇順連結（枠数非依存・順序非依存）。 */
export function buildIdOf(solution: Solution): string {
  return solution.modules
    .map((m) => m.uuid)
    .sort((a, b) => a - b)
    .join("-");
}

// 既定のビルド名（保存時刻ベース）。後からユーザーが変更できる。
// 接頭辞は表示言語に追従させる（保存済みの名前はユーザーの持ち物なので後から書き換えない）。
function defaultName(prefix: string): string {
  const d = new Date();
  const p = (n: number) => String(n).padStart(2, "0");
  return `${prefix} ${p(d.getMonth() + 1)}/${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
}

export interface UseFavorites {
  favorites: SavedBuild[];
  isFavorite: (solution: Solution) => boolean;
  /** 未保存なら追加、保存済みなら削除（トグル）。 */
  toggle: (solution: Solution) => void;
  rename: (id: string, name: string) => void;
  remove: (id: string) => void;
}

// 擬似キャラ0（未特定。バックエンドの owned_modules.json レガシー単一ファイルと同じ
// 位置付け）は、キャラ別キーを新設せずレガシーお気に入りキーそのものを指す。こうしないと
// 「起動直後は擬似キャラ0が有効→レガシーお気に入りが実キャラ用キーへ移行される→
// 後で擬似キャラ0を再度表示しても favorites.v1.0 は存在せず空」という、擬似キャラ0
// 経由での参照が picker から永久に外れて中身が迷子になる事故を招く。
function favoritesStorageKey(charId: string): string {
  return charId === "0" ? STORAGE_KEYS.favorites : `${STORAGE_KEYS.favorites}.${charId}`;
}

/** 純粋な読込のみ（副作用なし）。移行の要否判定・実行は別途 effect 側で行う。 */
function loadFavoritesRaw(charId: string): SavedBuild[] {
  return loadJSON<SavedBuild[]>(favoritesStorageKey(charId), []);
}

// レガシー→実キャラへの移行が必要か。擬似キャラ0はレガシー領域そのものを指すため
// 移行対象外。実キャラの中で最初に有効になった1人だけに限定する
// （`favoritesMigratedTo` に移行先の char_id を記録して以後はガードする）。
function needsLegacyMigration(charId: string): boolean {
  if (charId === "0") return false;
  if (hasKey(favoritesStorageKey(charId))) return false;
  if (!hasKey(STORAGE_KEYS.favorites)) return false;
  return loadJSON<string | null>(STORAGE_KEYS.favoritesMigratedTo, null) == null;
}

/**
 * お気に入りビルドの一覧と CRUD。キャラ別に localStorage へ永続化する。
 * charId が null（キャラ未検知・未解決）の間は読込・保存とも行わない
 * （"0" 等の暫定キーへゴミを作らないため）。
 */
export function useFavorites(charId: string | null): UseFavorites {
  const { t } = useI18n();
  const [favorites, setFavorites] = useState<SavedBuild[]>(() =>
    charId != null ? loadFavoritesRaw(charId) : [],
  );
  // charId 切替直後の1回だけ自動保存をスキップする（別キャラの一覧をまだ切替中の
  // charId のキーへ保存し戻してしまう競合を防ぐ）。
  const skipNextSaveRef = useRef(false);

  useEffect(() => {
    skipNextSaveRef.current = true;
    setFavorites(charId != null ? loadFavoritesRaw(charId) : []);
  }, [charId]);

  // レガシー→実キャラへの移行（副作用）。読込 effect とは別に、必要な時だけ実行する
  // （loadFavoritesRaw を副作用フリーに保つ。移行判定・実行を state 初期化関数の中で
  // 行うと、将来 charId 非 null で直接マウントされた場合に「1回目が移行してフラグを
  // 立て、2回目の呼び出しは既に移行済みとして空を返す」という同種の消失が起こりうる）。
  useEffect(() => {
    if (charId == null || !needsLegacyMigration(charId)) return;
    const legacy = loadJSON<SavedBuild[]>(STORAGE_KEYS.favorites, []);
    const key = favoritesStorageKey(charId);
    saveJSON(key, legacy);
    // 保存が実際に成功したことを確認してからフラグを立てる（quota 超過等で保存に
    // 失敗した場合、フラグだけ立って内容は空のまま＝恒久的な喪失になるのを防ぐ）。
    if (hasKey(key)) {
      saveJSON(STORAGE_KEYS.favoritesMigratedTo, charId);
      setFavorites(legacy);
    }
  }, [charId]);

  useEffect(() => {
    if (skipNextSaveRef.current) {
      skipNextSaveRef.current = false;
      return;
    }
    if (charId == null) return;
    saveJSON(favoritesStorageKey(charId), favorites);
  }, [favorites, charId]);

  const ids = useMemo(() => new Set(favorites.map((f) => f.id)), [favorites]);

  const isFavorite = useCallback(
    (solution: Solution) => ids.has(buildIdOf(solution)),
    [ids],
  );

  const toggle = useCallback(
    (solution: Solution) => {
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
          {
            id,
            name: defaultName(t("fav.namePrefix")),
            solution,
            targetIds,
            savedAt: Date.now(),
          },
        ];
      });
    },
    [t],
  );

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
