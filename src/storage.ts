// localStorage の型付きヘルパ。キーは名前空間 + バージョンで管理する。
// 将来 Tauri のファイル保存へ差し替えられるよう、読み書きはここに集約する。

const PREFIX = "bpsr.";

export const STORAGE_KEYS = {
  presets: "presets.v1",
  favorites: "favorites.v1",
  lastSearch: "lastSearch.v1",
  lang: "lang.v1",
  footerVisible: "footerVisible.v1",
} as const;

/** JSON を読み込む。未保存・破損時は fallback を返し、破損は警告ログに残す。 */
export function loadJSON<T>(key: string, fallback: T): T {
  let raw: string | null;
  try {
    raw = localStorage.getItem(PREFIX + key);
  } catch (e) {
    console.warn(`[storage] 読み込み失敗 (${key}):`, e);
    return fallback;
  }
  if (raw == null) return fallback;
  try {
    return JSON.parse(raw) as T;
  } catch (e) {
    console.warn(`[storage] JSON 解析失敗 (${key})。fallback を使用:`, e);
    return fallback;
  }
}

/** JSON を保存する。容量超過などの失敗は握り潰さず警告ログに残す。 */
export function saveJSON<T>(key: string, value: T): void {
  try {
    localStorage.setItem(PREFIX + key, JSON.stringify(value));
  } catch (e) {
    console.warn(`[storage] 保存失敗 (${key}):`, e);
  }
}
