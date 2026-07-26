// localStorage の型付きヘルパ。キーは名前空間 + バージョンで管理する。
// 将来 Tauri のファイル保存へ差し替えられるよう、読み書きはここに集約する。

const PREFIX = "bpsr.";

export const STORAGE_KEYS = {
  presets: "presets.v1",
  // レガシー・単一キャラ時代のお気に入りキー。char_id="0"（未特定の擬似キャラ。
  // バックエンドの owned_modules.json と同じ扱い）は常にこのキーそのものを指す
  // （favorites.v1.0 は新設しない）。実キャラ用のキー生成は useFavorites 参照。
  favorites: "favorites.v1",
  // レガシーお気に入りを最初に引き継いだ実キャラの char_id（未移行なら未保存）。
  // 「最初の1キャラのみへ移行」のガードとして使う。値は保存が成功した後にのみ書く。
  favoritesMigratedTo: "favoritesMigratedTo.v1",
  lastSearch: "lastSearch.v1",
  lang: "lang.v1",
  footerVisible: "footerVisible.v1",
  selectedCharId: "selectedCharId.v1",
} as const;

/** キーが localStorage に存在するか（移行要否の判定用）。 */
export function hasKey(key: string): boolean {
  try {
    return localStorage.getItem(PREFIX + key) != null;
  } catch (e) {
    console.warn(`[storage] 存在確認失敗 (${key}):`, e);
    return false;
  }
}

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
