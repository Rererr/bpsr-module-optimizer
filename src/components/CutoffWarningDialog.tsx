import { useEffect } from "react";
import { AlertTriangle } from "lucide-react";
import { useI18n } from "../i18n";

interface Props {
  /** 適用しようとしている品質の下限（0=絞り込みなし）。 */
  quality: number;
  /** 適用しようとしている属性値合計の下限（0=絞り込みなし）。 */
  value: number;
  onConfirm: () => void;
  onCancel: () => void;
}

/**
 * 候補の切り捨てを有効化するときの確認ダイアログ。
 * 切り捨ては探索を速くする代わりに「所持モジュール全体での最適な組み合わせ」を
 * 取り逃がしうるため、有効化の操作を一度止めて明示的に承諾させる。
 */
export function CutoffWarningDialog({ quality, value, onConfirm, onCancel }: Props) {
  const { t } = useI18n();

  // Escape で閉じる（オーバーレイのクリックと同じくキャンセル扱い）。
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") onCancel();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onCancel]);

  const conds = [
    quality > 0
      ? t("cutoff.condQuality", { label: t(`cutoff.quality${quality}`) })
      : null,
    value > 0 ? t("cutoff.condValue", { v: value }) : null,
  ].filter(Boolean);

  return (
    <div
      className="fixed inset-0 z-50 flex items-center justify-center bg-slate-950/70 p-4"
      onClick={onCancel}
      role="presentation"
    >
      <div
        role="alertdialog"
        aria-modal="true"
        aria-labelledby="cutoff-warn-title"
        className="w-full max-w-md rounded-xl border border-slate-700 bg-slate-900 p-5 shadow-2xl"
        onClick={(e) => e.stopPropagation()}
      >
        <div className="mb-3 flex items-center gap-2 text-amber-400">
          <AlertTriangle size={18} className="shrink-0" />
          <h2 id="cutoff-warn-title" className="text-sm font-bold">
            {t("cutoff.warnTitle")}
          </h2>
        </div>
        <p className="mb-3 text-sm leading-relaxed text-slate-200">{t("cutoff.warnBody")}</p>
        <p className="mb-3 rounded-lg bg-slate-800/60 px-3 py-2 text-xs leading-relaxed text-slate-300">
          {t("cutoff.warnDetail", { cond: conds.join(" / ") })}
        </p>
        {/* 金品質のみへの絞り込みは実測で最も壊れやすい（5パターン中4件で1位が変化）。
            他の条件より一段強い注意を出す。 */}
        {quality >= 4 && (
          <p className="mb-4 rounded-lg border border-amber-700/50 bg-amber-500/10 px-3 py-2 text-xs leading-relaxed text-amber-200">
            {t("cutoff.warnQuality4")}
          </p>
        )}
        <div className="flex justify-end gap-2">
          <button
            onClick={onCancel}
            className="h-9 rounded-lg border border-slate-700 px-4 text-xs font-medium text-slate-300 transition hover:bg-slate-800"
          >
            {t("common.cancel")}
          </button>
          <button
            autoFocus
            onClick={onConfirm}
            className="h-9 rounded-lg bg-amber-600 px-4 text-xs font-bold text-white shadow transition hover:bg-amber-500"
          >
            {t("cutoff.warnConfirm")}
          </button>
        </div>
      </div>
    </div>
  );
}
