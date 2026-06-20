import { type ReactNode, useEffect, useRef, useState } from "react";
import { Check } from "lucide-react";

interface Props {
  onConfirm: () => void;
  /** aria-label / title に使う説明（例: 「プリセットを削除」）。 */
  label: string;
  /** アイドル時に表示するアイコン。 */
  idle: ReactNode;
  /** アイドル時の追加クラス（色など）。 */
  className?: string;
}

/**
 * 2段階確認ボタン。1回目で警告状態（rose + チェックアイコン）に変わり、
 * 1.5秒以内の2回目で onConfirm を実行する。誤クリックによる即時削除を防ぐ。
 */
export function ConfirmButton({ onConfirm, label, idle, className = "" }: Props) {
  const [armed, setArmed] = useState(false);
  const timer = useRef<number | null>(null);

  useEffect(
    () => () => {
      if (timer.current) clearTimeout(timer.current);
    },
    [],
  );

  const click = () => {
    if (armed) {
      if (timer.current) clearTimeout(timer.current);
      setArmed(false);
      onConfirm();
      return;
    }
    setArmed(true);
    if (timer.current) clearTimeout(timer.current);
    timer.current = window.setTimeout(() => setArmed(false), 1500);
  };

  return (
    <button
      onClick={click}
      aria-label={armed ? `${label}（もう一度押して確定）` : label}
      title={armed ? "もう一度押して削除" : label}
      className={`shrink-0 rounded-md p-1 transition ${
        armed ? "bg-rose-500/15 text-rose-300" : className
      }`}
    >
      {armed ? <Check size={13} /> : idle}
    </button>
  );
}
