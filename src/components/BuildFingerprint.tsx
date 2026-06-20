import type { Module } from "../types";

// モジュール品質 → 塗り色（SolutionCard の品質配色と対応）。
function qualityFill(q: number): string {
  switch (q) {
    case 4:
      return "bg-purple-400";
    case 3:
      return "bg-sky-400";
    case 2:
      return "bg-emerald-400";
    default:
      return "bg-slate-500";
  }
}

interface Props {
  modules: Module[];
  className?: string;
}

/**
 * ビルド指紋ストリップ。4モジュールの品質色を等幅セグメントで並べ、
 * 保存ビルドを一目で識別できるようにする。
 */
export function BuildFingerprint({ modules, className = "" }: Props) {
  return (
    <span
      className={`inline-flex h-1.5 overflow-hidden rounded-full ${className}`}
      title={modules.map((m) => `${m.name} (Q${m.quality})`).join(" / ")}
    >
      {modules.map((m) => (
        <span key={m.key} className={`h-full w-3 ${qualityFill(m.quality)}`} />
      ))}
    </span>
  );
}
