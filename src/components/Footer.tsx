import { openUrl } from "@tauri-apps/plugin-opener";
import { getVersion } from "@tauri-apps/api/app";
import { X } from "lucide-react";
import { useI18n } from "../i18n";

const GITHUB_ISSUE_URL = "https://github.com/Rererr/bpsr-module-optimizer/issues/new";

interface Props {
  onHide: () => void;
}

// 既定ブラウザで外部リンクを開く。webview 内 window.open は Tauri v2 では使わず opener プラグイン経由。
// 失敗してもフッター表示自体は継続させたいため、UI へは伝播させずログのみ残す。
async function openExternal(url: string, label: string): Promise<void> {
  try {
    await openUrl(url);
  } catch (e) {
    console.error(`[Footer] ${label} のオープンに失敗:`, e);
  }
}

export function Footer({ onHide }: Props) {
  const { t } = useI18n();

  const handleContact = async () => {
    let version = "unknown";
    try {
      version = await getVersion();
    } catch (e) {
      console.error("[Footer] バージョン取得に失敗:", e);
    }
    const url = `https://rererr-portfolio.pages.dev/?from=bpsr-module-optimizer&v=${encodeURIComponent(version)}#contact`;
    await openExternal(url, t("footer.contact"));
  };

  const handleReport = () => openExternal(GITHUB_ISSUE_URL, t("footer.reportGithub"));

  return (
    <footer className="flex shrink-0 items-center justify-end gap-3 border-t border-slate-800 bg-slate-900/60 px-5 py-1 text-xs opacity-50">
      <button
        onClick={handleReport}
        className="text-slate-300 underline-offset-2 hover:text-slate-100 hover:underline"
      >
        {t("footer.reportGithub")}
      </button>
      <button
        onClick={handleContact}
        className="text-slate-300 underline-offset-2 hover:text-slate-100 hover:underline"
      >
        {t("footer.contact")}
      </button>
      <button
        onClick={onHide}
        aria-label={t("footer.hide")}
        title={t("footer.hide")}
        className="text-slate-400 hover:text-slate-100"
      >
        <X size={12} />
      </button>
    </footer>
  );
}
