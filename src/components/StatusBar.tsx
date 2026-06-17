import { RefreshCw, Radio, FileJson, CircleAlert } from "lucide-react";
import type { StatusDto } from "../types";

interface Props {
  status: StatusDto | null;
  onReloadDump: () => void;
  busy: boolean;
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

function stateLabel(s: StatusDto["capture_state"]): string {
  switch (s) {
    case "running":
      return "キャプチャ稼働中";
    case "failed":
      return "キャプチャ失敗(要管理者権限)";
    default:
      return "キャプチャ待機";
  }
}

function ago(ms: number | null): string {
  if (ms == null) return "未取得";
  const d = Date.now() - ms;
  if (d < 1000) return "たった今";
  if (d < 60000) return `${Math.floor(d / 1000)}秒前`;
  if (d < 3600000) return `${Math.floor(d / 60000)}分前`;
  return `${Math.floor(d / 3600000)}時間前`;
}

export function StatusBar({ status, onReloadDump, busy }: Props) {
  return (
    <header className="flex items-center justify-between gap-4 border-b border-slate-800 bg-slate-900/60 px-5 py-3 backdrop-blur">
      <div className="flex items-center gap-3">
        <div className="flex h-8 w-8 items-center justify-center rounded-lg bg-indigo-500/20 text-indigo-300">
          <Radio size={18} />
        </div>
        <div>
          <h1 className="text-sm font-bold tracking-wide text-slate-100">
            BPSR モジュール最適化
          </h1>
          <p className="text-[11px] text-slate-400">
            Lv6を優先しリンク効果が最大になる4枠を探索
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
          <span>件</span>
          <span className="text-slate-600">·</span>
          <span>{ago(status?.last_update_ms ?? null)}</span>
        </div>

        <button
          onClick={onReloadDump}
          disabled={busy}
          className="flex items-center gap-1.5 rounded-md border border-slate-700 bg-slate-800 px-2.5 py-1.5 text-slate-200 transition hover:bg-slate-700 disabled:opacity-50"
          title="owned_modules.json を再読込"
        >
          <RefreshCw size={13} className={busy ? "animate-spin" : ""} />
          ダンプ再読込
        </button>
      </div>
    </header>
  );
}
