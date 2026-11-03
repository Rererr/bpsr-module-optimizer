import { useState } from "react";
import { ExternalLink, TriangleAlert } from "lucide-react";
import { useI18n } from "../i18n";
import type { ValueIssueDto } from "../types";
import { GITHUB_RELEASES_URL, appVersion, issueUrl, openExternal } from "../external";

interface Props {
  issue: ValueIssueDto;
  // 有効キャラの現在のモジュール件数。missing は一覧を更新しないため、
  // 0 件なら「前回のデータ」は存在せず、探索もできない。
  moduleCount: number;
}

// 属性値の取得異常（ゲーム更新で通信形式が変わった等）の告知。データ自体の問題なので
// タブに関わらず表示し、次の行動（最新版の確認 → 無ければ報告）へ直接つなぐ。
// フッターは非表示にできるため、報告の導線はここにも置く。
export function ValueIssueBanner({ issue, moduleCount }: Props) {
  const { t } = useI18n();

  const title =
    issue.kind === "missing" ? t("valueIssue.missingTitle") : t("valueIssue.partialTitle");
  const detail =
    issue.kind === "missing"
      ? moduleCount > 0
        ? t("valueIssue.missingDetailStale", { total: issue.total })
        : t("valueIssue.missingDetailEmpty", { total: issue.total })
      : t("valueIssue.partialDetail", { affected: issue.affected, total: issue.total });

  // 開いている最中の連打で同じタブを複数開かないためのガード。disabled にすると
  // キーボード操作中のフォーカスが body へ落ちるので、aria-disabled と open 先頭の
  // 早期リターンで止める。
  const [opening, setOpening] = useState(false);
  // 開けなかった URL。ブラウザへ手で貼れるよう画面に出す。
  const [failedUrl, setFailedUrl] = useState<string | null>(null);

  const open = async (buildUrl: () => Promise<string>, label: string) => {
    if (opening) return;
    setOpening(true);
    setFailedUrl(null);
    try {
      const url = await buildUrl();
      if (!(await openExternal(url, label))) setFailedUrl(url);
    } finally {
      setOpening(false);
    }
  };

  const reportUrl = async () => {
    const version = await appVersion();
    const counts =
      issue.kind === "missing"
        ? `total: ${issue.total}`
        : `affected: ${issue.affected} / total: ${issue.total}`;
    const body = [
      "<!-- Auto-filled by the app. Add anything you noticed below. -->",
      `- app version: ${version}`,
      `- kind: ${issue.kind}`,
      `- ${counts}`,
      "",
    ].join("\n");
    // タイトルは UI 言語に依らない形にして、Issue 検索で拾いやすくする。
    return issueUrl(`[value_issue] ${issue.kind}`, body);
  };

  return (
    <div
      role="alert"
      className="mb-3 flex items-start gap-2 rounded-lg border border-amber-600/60 bg-amber-500/15 px-4 py-2.5 text-sm text-amber-100"
    >
      <TriangleAlert size={16} className="mt-0.5 shrink-0 text-amber-300" />
      <div className="min-w-0 flex-1">
        <p className="font-semibold">{title}</p>
        <p className="mt-0.5 text-xs leading-relaxed text-amber-100/90">{detail}</p>
        <div className="mt-2 flex flex-wrap gap-2">
          <button
            type="button"
            aria-disabled={opening}
            onClick={() =>
              open(async () => GITHUB_RELEASES_URL, t("valueIssue.checkRelease"))
            }
            className="inline-flex items-center gap-1 rounded-md border border-amber-500/50 px-2.5 py-1 text-xs font-medium text-amber-100 hover:bg-amber-500/20 aria-disabled:opacity-60"
          >
            {t("valueIssue.checkRelease")}
            <ExternalLink size={12} aria-hidden="true" />
          </button>
          <button
            type="button"
            aria-disabled={opening}
            onClick={() => open(reportUrl, t("footer.reportGithub"))}
            className="inline-flex items-center gap-1 rounded-md bg-amber-500 px-2.5 py-1 text-xs font-semibold text-slate-950 hover:bg-amber-400 aria-disabled:opacity-60"
          >
            {t("footer.reportGithub")}
            <ExternalLink size={12} aria-hidden="true" />
          </button>
        </div>
        {failedUrl && (
          <p className="mt-2 break-all text-xs text-amber-100/90">
            {t("valueIssue.openFailed")}{" "}
            <span className="select-all font-mono">{failedUrl}</span>
          </p>
        )}
      </div>
    </div>
  );
}
