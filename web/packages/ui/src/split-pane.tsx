import { useRef, useState } from "react";
import type { KeyboardEvent, PointerEvent, ReactNode } from "react";

const clamp = (v: number, min: number, max: number) => Math.min(Math.max(v, min), max);

/** Two panes with a divider that moves by pointer or arrow keys. */
export const SplitPane = ({ label, start, end, initial = 50, min = 20, max = 80 }: { readonly label: string; readonly start: ReactNode; readonly end: ReactNode; readonly initial?: number; readonly min?: number; readonly max?: number }) => {
  const [percent, setPercent] = useState(initial);
  const box = useRef<HTMLDivElement>(null);
  const onMove = (e: PointerEvent<HTMLDivElement>) => {
    if (!e.currentTarget.hasPointerCapture(e.pointerId) || box.current === null) return;
    const r = box.current.getBoundingClientRect();
    setPercent(clamp(((e.clientX - r.left) / r.width) * 100, min, max));
  };
  const onKey = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key === "ArrowLeft") setPercent((p) => clamp(p - 2, min, max));
    if (e.key === "ArrowRight") setPercent((p) => clamp(p + 2, min, max));
  };
  return (
    <div ref={box} className="flex h-full w-full overflow-hidden rounded-md border border-border">
      <div style={{ width: `${percent}%` }} className="overflow-auto p-3">{start}</div>
      <div
        role="separator"
        aria-label={label}
        aria-orientation="vertical"
        aria-valuenow={Math.round(percent)}
        aria-valuemin={min}
        aria-valuemax={max}
        tabIndex={0}
        onKeyDown={onKey}
        onPointerDown={(e) => e.currentTarget.setPointerCapture(e.pointerId)}
        onPointerMove={onMove}
        className="w-1.5 shrink-0 cursor-col-resize bg-border-strong hover:bg-accent"
      />
      <div className="flex-1 overflow-auto p-3">{end}</div>
    </div>
  );
};
