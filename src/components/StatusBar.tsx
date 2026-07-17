import { RefreshCw, Radio, FileJson, CircleAlert, Languages, Eye } from "lucide-react";
import type { StatusDto } from "../types";
import { useI18n } from "../i18n";

interface Props {
  status: StatusDto | null;
  onReloadDump: () => void;
  busy: boolean;
  slotCount: number;
  // フッターの ×（フッター側にある閉じるボタン）で非表示にした後、再表示するための唯一の導線。
  // 設定画面が無いため、フッターが非表示のときだけヘッダーにこのボタンを出す。
  footerVisible: boolean;
  onShowFooter: () => void;
}

function stateColor(s: StatusDto["capture_state"]): string {
  switch (s) {
    case "running":
      return "bg-emerald-500";
    case "failed":
      return "bg-rose-500";
    default:
      return "bg-amber-400";
  }
}

export function StatusBar({
  status,
  onReloadDump,
  busy,
  slotCount,
  footerVisible,
  onShowFooter,
}: Props) {
  const { t, lang, setLang } = useI18n();

  const stateLabel = (s: StatusDto["capture_state"]): string => {
    switch (s) {
      case "running":
        return t("status.running");
      case "failed":
        return t("status.failed");
      default:
        return t("status.waiting");
    }
  };

  const ago = (ms: number | null): string => {
    if (ms == null) return t("time.never");
    const d = Date.now() - ms;
    if (d < 1000) return t("time.now");
    if (d < 60000) return t("time.secAgo", { n: Math.floor(d / 1000) });
    if (d < 3600000) return t("time.minAgo", { n: Math.floor(d / 60000) });
    return t("time.hourAgo", { n: Math.floor(d / 3600000) });
  };

  return (
    <header className="flex items-center justify-between gap-4 border-b border-slate-800 bg-slate-900/60 px-5 py-3 backdrop-blur">
      <div className="flex items-center gap-3">
        <div className="flex h-8 w-8 items-center justify-center rounded-lg bg-indigo-500/20 text-indigo-300">
          <Radio size={18} />
        </div>
        <div>
          <h1 className="text-sm font-bold tracking-wide text-slate-100">
            {t("app.title")}
          </h1>
          <p className="text-[11px] text-slate-400">
            {t("app.subtitle", { n: slotCount })}
          </p>
        </div>
      </div>

      <div className="flex items-center gap-4 text-xs">
        <div className="flex items-center gap-2">
          <span
            className={`inline-block h-2.5 w-2.5 rounded-full ${stateColor(
              status?.capture_state ?? "init",
            )} ${status?.capture_state === "running" ? "animate-pulse" : ""}`}
          />
          <span className="text-slate-300">
            {stateLabel(status?.capture_state ?? "init")}
          </span>
        </div>

        <div className="flex items-center gap-1.5 text-slate-400">
          {status?.source === "dump" ? (
            <FileJson size={14} className="text-sky-400" />
          ) : status?.source === "capture" ? (
            <Radio size={14} className="text-emerald-400" />
          ) : (
            <CircleAlert size={14} className="text-slate-500" />
          )}
          <span className="font-semibold text-slate-200">
            {status?.module_count ?? 0}
          </span>
          <span>{t("status.modulesUnit")}</span>
          <span className="text-slate-600">·</span>
          <span>{ago(status?.last_update_ms ?? null)}</span>
        </div>

        <button
          onClick={onReloadDump}
          disabled={busy}
          className="flex items-center gap-1.5 rounded-md border border-slate-700 bg-slate-800 px-2.5 py-1.5 text-slate-200 transition hover:bg-slate-700 disabled:opacity-50"
          title={t("status.reloadTitle")}
        >
          <RefreshCw size={13} className={busy ? "animate-spin" : ""} />
          {t("status.reload")}
        </button>

        <button
          onClick={() => setLang(lang === "ja" ? "en" : "ja")}
          aria-label={t("lang.switch")}
          title={t("lang.switch")}
          className="flex items-center gap-1.5 rounded-md border border-slate-700 bg-slate-800 px-2.5 py-1.5 font-semibold text-slate-200 transition hover:bg-slate-700"
        >
          <Languages size={13} />
          {lang === "ja" ? "EN" : "日本語"}
        </button>

        {!footerVisible && (
          <button
            onClick={onShowFooter}
            aria-label={t("footer.show")}
            title={t("footer.show")}
            className="flex items-center gap-1.5 rounded-md border border-slate-700 bg-slate-800 px-2.5 py-1.5 text-slate-200 transition hover:bg-slate-700"
          >
            <Eye size={13} />
          </button>
        )}
      </div>
    </header>
  );
}
