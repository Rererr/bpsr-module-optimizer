import { openUrl } from "@tauri-apps/plugin-opener";
import { getVersion } from "@tauri-apps/api/app";

const REPO_URL = "https://github.com/Rererr/bpsr-module-optimizer";
export const GITHUB_ISSUE_URL = `${REPO_URL}/issues/new`;
export const GITHUB_RELEASES_URL = `${REPO_URL}/releases/latest`;

// 既定ブラウザで外部リンクを開く。webview 内 window.open は Tauri v2 では使わず opener プラグイン経由。
// 例外は投げずに成否を返す（失敗はログに残す）。リンクを開くこと自体が目的の導線では、
// 呼び出し側が false を見て代替（URL の表示など）を出す。
export async function openExternal(url: string, label: string): Promise<boolean> {
  try {
    await openUrl(url);
    return true;
  } catch (e) {
    console.error(`[external] ${label} のオープンに失敗:`, e);
    return false;
  }
}

// アプリのバージョン。取得に失敗しても報告・問い合わせの導線は止めない。
export async function appVersion(): Promise<string> {
  try {
    return await getVersion();
  } catch (e) {
    console.error("[external] バージョン取得に失敗:", e);
    return "unknown";
  }
}

// タイトル・本文を事前入力した新規 Issue の URL。
export function issueUrl(title: string, body: string): string {
  const q = new URLSearchParams({ title, body });
  return `${GITHUB_ISSUE_URL}?${q.toString()}`;
}
