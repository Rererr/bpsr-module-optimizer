import { useMemo, useState } from "react";
import { Star, Pencil, Trash2, Check, X, GitCompare } from "lucide-react";
import type { SavedBuild } from "../types";
import { SolutionCard } from "./SolutionCard";
import { BuildFingerprint } from "./BuildFingerprint";
import { BuildCompare } from "./BuildCompare";
import { ConfirmButton } from "./ConfirmButton";
import { useI18n } from "../i18n";

interface Props {
  favorites: SavedBuild[];
  onRename: (id: string, name: string) => void;
  onRemove: (id: string) => void;
}

const MAX_COMPARE = 3;

export function FavoritesPanel({ favorites, onRename, onRemove }: Props) {
  const { t } = useI18n();
  const [compareIds, setCompareIds] = useState<Set<string>>(new Set());
  const [comparing, setComparing] = useState(false);
  const [editingId, setEditingId] = useState<string | null>(null);
  const [draft, setDraft] = useState("");

  const selectedBuilds = useMemo(
    () => favorites.filter((f) => compareIds.has(f.id)),
    [favorites, compareIds],
  );

  if (favorites.length === 0) {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-3 text-center text-slate-500">
        <Star size={48} className="opacity-40" />
        <div>
          <p className="text-sm">{t("fav.emptyTitle")}</p>
          <p className="mt-1 text-xs">{t("fav.emptyDesc")}</p>
        </div>
      </div>
    );
  }

  const toggleCompare = (id: string) => {
    setCompareIds((prev) => {
      const next = new Set(prev);
      if (next.has(id)) next.delete(id);
      else if (next.size < MAX_COMPARE) next.add(id);
      return next;
    });
  };

  const startEdit = (b: SavedBuild) => {
    setEditingId(b.id);
    setDraft(b.name);
  };
  const commitEdit = () => {
    if (editingId) onRename(editingId, draft);
    setEditingId(null);
    setDraft("");
  };

  return (
    <div className="flex flex-col gap-3">
      {/* 比較ツールバー */}
      <div className="flex flex-wrap items-center gap-2 text-xs text-slate-400">
        <button
          onClick={() => setComparing(true)}
          disabled={compareIds.size < 2}
          className="flex items-center gap-1.5 rounded-lg bg-indigo-500 px-3 py-1.5 font-medium text-white transition hover:bg-indigo-400 disabled:cursor-not-allowed disabled:bg-slate-700 disabled:text-slate-400"
        >
          <GitCompare size={13} />
          {t("fav.compareN", { n: compareIds.size })}
        </button>
        {compareIds.size > 0 && (
          <button
            onClick={() => setCompareIds(new Set())}
            className="text-slate-400 underline-offset-2 hover:text-slate-200 hover:underline"
          >
            {t("fav.clearSel")}
          </button>
        )}
        {compareIds.size === 0 && (
          <span className="text-slate-500">{t("fav.compareHint")}</span>
        )}
        {compareIds.size === 1 && (
          <span className="text-amber-300">{t("fav.compareOneMore")}</span>
        )}
      </div>

      {comparing && selectedBuilds.length >= 2 && (
        <BuildCompare builds={selectedBuilds} onClose={() => setComparing(false)} />
      )}

      <div className="grid grid-cols-1 gap-3 lg:grid-cols-2 2xl:grid-cols-3">
        {favorites.map((b) => {
          const checked = compareIds.has(b.id);
          const atLimit = compareIds.size >= MAX_COMPARE && !checked;
          return (
            <div key={b.id} className="flex flex-col gap-1.5">
              {/* カードヘッダ: 比較選択 + 指紋 + 名前 + 編集/削除 */}
              <div className="flex items-center gap-2 px-1">
                <label
                  className={`flex items-center gap-1 text-[11px] ${
                    atLimit ? "cursor-not-allowed text-slate-600" : "cursor-pointer text-slate-400"
                  }`}
                  title={atLimit ? t("fav.compareMax") : t("fav.compareAdd")}
                >
                  <input
                    type="checkbox"
                    checked={checked}
                    disabled={atLimit}
                    onChange={() => toggleCompare(b.id)}
                    aria-label={
                      t("fav.addToCompare", { name: b.name }) +
                      (atLimit ? t("fav.maxSuffix") : "")
                    }
                    className="accent-indigo-500"
                  />
                  {t("fav.compareLabel")}
                </label>

                <BuildFingerprint modules={b.solution.modules} />

                {editingId === b.id ? (
                  <div className="flex min-w-0 flex-1 items-center gap-1">
                    <input
                      autoFocus
                      value={draft}
                      onChange={(e) => setDraft(e.currentTarget.value)}
                      onKeyDown={(e) => {
                        if (e.key === "Enter") commitEdit();
                        else if (e.key === "Escape") {
                          setEditingId(null);
                          setDraft("");
                        }
                      }}
                      className="min-w-0 flex-1 rounded-md border border-indigo-500/60 bg-slate-900 px-1.5 py-0.5 text-xs text-slate-100 outline-none"
                    />
                    <button
                      onClick={commitEdit}
                      aria-label={t("fav.saveName")}
                      title={t("common.save")}
                      className="rounded p-1 text-emerald-400 transition hover:bg-slate-800"
                    >
                      <Check size={13} />
                    </button>
                    <button
                      onClick={() => {
                        setEditingId(null);
                        setDraft("");
                      }}
                      aria-label={t("fav.cancelEdit")}
                      title={t("common.cancel")}
                      className="rounded p-1 text-slate-400 transition hover:bg-slate-800"
                    >
                      <X size={13} />
                    </button>
                  </div>
                ) : (
                  <>
                    <span
                      className="min-w-0 flex-1 truncate text-xs font-semibold text-slate-200"
                      title={b.name}
                    >
                      {b.name}
                    </span>
                    <button
                      onClick={() => startEdit(b)}
                      aria-label={t("fav.renameName", { name: b.name })}
                      title={t("fav.rename")}
                      className="rounded p-1 text-slate-500 transition hover:bg-slate-800 hover:text-slate-200"
                    >
                      <Pencil size={13} />
                    </button>
                    <ConfirmButton
                      onConfirm={() => onRemove(b.id)}
                      label={t("fav.deleteName", { name: b.name })}
                      idle={<Trash2 size={13} />}
                      className="text-slate-500 hover:bg-slate-800 hover:text-rose-300"
                    />
                  </>
                )}
              </div>

              {/* targetAttrs は渡さない: 保存時の targetIds は breakdown の selected かつ level>=1
                  （＝存在した目標のみ）由来で total === selected_present となり「一部含められず」
                  チップは構造上出ないため。名前列挙用メタも不要。 */}
              <SolutionCard
                solution={b.solution}
                targetIds={new Set(b.targetIds)}
              />
            </div>
          );
        })}
      </div>
    </div>
  );
}
