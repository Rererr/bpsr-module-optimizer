import { useState } from "react";
import { Plus, X, Check } from "lucide-react";
import type { SearchPreset } from "../types";
import { ConfirmButton } from "./ConfirmButton";

interface Props {
  presets: SearchPreset[];
  onApply: (preset: SearchPreset) => void;
  onSave: (name: string) => void;
  onDelete: (id: string) => void;
  /** 現在の条件が保存に値するか（デフォルトのままなら無効化）。 */
  canSave: boolean;
}

function counts(selection: SearchPreset["selection"]) {
  let target = 0;
  let exclude = 0;
  for (const s of Object.values(selection)) {
    if (s === "target") target++;
    else if (s === "exclude") exclude++;
  }
  return { target, exclude };
}

export function PresetBar({ presets, onApply, onSave, onDelete, canSave }: Props) {
  const [naming, setNaming] = useState(false);
  const [draft, setDraft] = useState("");

  const commit = () => {
    const name = draft.trim();
    if (!name) return;
    onSave(name);
    setDraft("");
    setNaming(false);
  };

  return (
    <div className="flex flex-col gap-2">
      {presets.length === 0 && !naming && (
        <p className="text-[11px] leading-relaxed text-slate-500">
          条件を保存しておくと、ワンクリックで呼び出せます。
        </p>
      )}

      {presets.length > 0 && (
        <div className="flex flex-col gap-1.5">
          {presets.map((p) => {
            const { target, exclude } = counts(p.selection);
            return (
              <div
                key={p.id}
                className="group flex items-center gap-1 rounded-lg border border-slate-700 bg-slate-800/60 transition hover:border-indigo-500/60"
              >
                <button
                  onClick={() => onApply(p)}
                  className="flex min-w-0 flex-1 items-center justify-between gap-2 px-2.5 py-1.5 text-left"
                  title={`「${p.name}」を適用`}
                >
                  <span className="truncate text-xs font-medium text-slate-200">
                    {p.name}
                  </span>
                  <span className="flex shrink-0 items-center gap-1.5 text-[10px]">
                    {target > 0 && (
                      <span className="text-emerald-300">目標{target}</span>
                    )}
                    {exclude > 0 && (
                      <span className="text-rose-300">除外{exclude}</span>
                    )}
                  </span>
                </button>
                <ConfirmButton
                  onConfirm={() => onDelete(p.id)}
                  label={`プリセット「${p.name}」を削除`}
                  idle={<X size={13} />}
                  className="text-slate-500 hover:bg-slate-700 hover:text-rose-300"
                />
              </div>
            );
          })}
        </div>
      )}

      {naming ? (
        <div className="flex items-center gap-1">
          <input
            autoFocus
            value={draft}
            onChange={(e) => setDraft(e.currentTarget.value)}
            onKeyDown={(e) => {
              if (e.key === "Enter") commit();
              else if (e.key === "Escape") {
                setNaming(false);
                setDraft("");
              }
            }}
            placeholder="プリセット名"
            className="min-w-0 flex-1 rounded-md border border-indigo-500/60 bg-slate-900 px-2 py-1.5 text-xs text-slate-100 outline-none placeholder:text-slate-600"
          />
          <button
            onClick={commit}
            disabled={!draft.trim()}
            aria-label="プリセットを保存"
            title="保存"
            className="shrink-0 rounded-md border border-emerald-600 bg-emerald-600/90 p-1.5 text-white transition hover:bg-emerald-500 disabled:cursor-not-allowed disabled:opacity-40"
          >
            <Check size={13} />
          </button>
          <button
            onClick={() => {
              setNaming(false);
              setDraft("");
            }}
            aria-label="保存をキャンセル"
            title="キャンセル"
            className="shrink-0 rounded-md border border-slate-700 p-1.5 text-slate-400 transition hover:bg-slate-700"
          >
            <X size={13} />
          </button>
        </div>
      ) : (
        <button
          onClick={() => setNaming(true)}
          disabled={!canSave}
          title={canSave ? "現在の条件を保存" : "保存できる条件がありません"}
          className="flex items-center justify-center gap-1.5 rounded-lg border border-dashed border-slate-700 px-2.5 py-1.5 text-xs text-slate-400 transition hover:border-slate-500 hover:text-slate-200 disabled:cursor-not-allowed disabled:opacity-40"
        >
          <Plus size={13} />
          現在の条件を保存
        </button>
      )}
    </div>
  );
}
