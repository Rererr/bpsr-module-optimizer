import {
  type ReactNode,
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import { CircleQuestionMark } from "lucide-react";

const PANEL_WIDTH = 320;
// ウィンドウ端との最小マージン。
const VIEWPORT_MARGIN = 12;

interface Props {
  /** アイコンの aria-label / title（例: 「属性の選び方」）。 */
  label: string;
  /** ポップアップの本文。 */
  children: ReactNode;
}

/**
 * 補足説明を隠すヘルプアイコン。ホバー・フォーカスで開き、クリックで固定できる。
 * 常時表示すると本来の操作要素が画面下へ押し出されるため、説明文をここへ退避する。
 *
 * パネルは position: fixed でトリガの位置から算出する。サイドバーは overflow-y-auto
 * （＝横方向も auto に計算される）ため、absolute のままだとサイドバー幅より広い
 * パネルが切れる。fixed なら祖先の overflow に切られず、アイコンをどこへ置いても
 * 同じ幅で開ける。
 */
export function HelpPopover({ label, children }: Props) {
  const [open, setOpen] = useState(false);
  const [pos, setPos] = useState<{ top: number; left: number } | null>(null);
  const id = useId();
  const ref = useRef<HTMLDivElement>(null);

  // トリガの中央にパネルを寄せ、ウィンドウ内へ収める。
  useLayoutEffect(() => {
    if (!open || !ref.current) return;
    const r = ref.current.getBoundingClientRect();
    const maxLeft = window.innerWidth - PANEL_WIDTH - VIEWPORT_MARGIN;
    setPos({
      top: r.bottom,
      left: Math.min(
        Math.max(VIEWPORT_MARGIN, r.left + r.width / 2 - PANEL_WIDTH / 2),
        Math.max(VIEWPORT_MARGIN, maxLeft),
      ),
    });
  }, [open]);

  // 外側クリックと Escape で閉じる。クリックで開いたままにした場合の唯一の閉じ方になる。
  useEffect(() => {
    if (!open) return;
    const onPointerDown = (e: MouseEvent) => {
      if (ref.current && !ref.current.contains(e.target as Node)) setOpen(false);
    };
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === "Escape") setOpen(false);
    };
    document.addEventListener("mousedown", onPointerDown);
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("mousedown", onPointerDown);
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [open]);

  return (
    <div
      ref={ref}
      className="relative"
      onMouseEnter={() => setOpen(true)}
      onMouseLeave={() => setOpen(false)}
    >
      <button
        type="button"
        aria-label={label}
        title={label}
        aria-expanded={open}
        aria-controls={open ? id : undefined}
        onClick={() => setOpen((v) => !v)}
        onFocus={() => setOpen(true)}
        className="flex h-5 w-5 items-center justify-center rounded-full text-slate-400 transition hover:bg-slate-700 hover:text-slate-100"
      >
        <CircleQuestionMark size={15} />
      </button>

      {open && pos && (
        // 外側の枠は上に余白を持たせたホバーの橋渡し用。パネルとの間に隙間を作ると、
        // マウスを移動する途中で mouseleave が発火して閉じてしまう。
        <div
          id={id}
          role="tooltip"
          className="fixed z-30 pt-2"
          style={{ top: pos.top, left: pos.left, width: PANEL_WIDTH }}
        >
          <div className="rounded-lg border border-slate-700 bg-slate-900 p-3 text-xs leading-relaxed text-slate-300 shadow-xl shadow-black/50">
            {children}
          </div>
        </div>
      )}
    </div>
  );
}
