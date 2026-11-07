import { activePart, formatClock, partsInRange, wordAt } from "@memex/core";
import type { TranscriptPart } from "@memex/types";
import { Badge, Button } from "@memex/ui";
import { memo, useEffect, useMemo, useRef, useState } from "react";

export interface TranscriptViewProps {
  readonly parts: readonly TranscriptPart[];
  readonly timeMs: number;
  /** A cited range to mark. */
  readonly fromMs: number | null;
  readonly toMs: number | null;
  readonly onSeek: (ms: number) => void;
  readonly hasMore: boolean;
  readonly loadingMore: boolean;
  readonly onLoadMore: () => void;
}

const Row = memo(({ part, active, inRange, timeMs, onSeek }: { readonly part: TranscriptPart; readonly active: boolean; readonly inRange: boolean; readonly timeMs: number | null; readonly onSeek: (ms: number) => void }) => {
  const current = timeMs === null ? null : wordAt(part.words, timeMs);
  return (
    <li data-part-active={active} data-part-in-range={inRange} aria-current={active ? "true" : undefined} className={`flex gap-2 rounded-sm p-1 ${active ? "bg-surface-selected" : ""} ${inRange ? "outline-1 outline-warning" : ""}`}>
      <Button variant="ghost" className="h-6 shrink-0 px-1 font-mono text-xs tabular-nums" aria-label={`Play from ${formatClock(part.tStartMs)}`} onClick={() => onSeek(part.tStartMs)}>{formatClock(part.tStartMs)}</Button>
      <div className="min-w-0 flex-1 text-sm">
        {part.speaker !== null && <span className="mr-1.5"><Badge>{part.speaker}</Badge></span>}
        {active && part.words.length > 0 ? (
          part.words.map((word, index) => (
            <span key={index}>
              <button type="button" onClick={() => onSeek(word.startMs)} className={`rounded-sm hover:underline ${current === index ? "bg-warning text-on-accent" : ""}`}>{word.text}</button>{" "}
            </span>
          ))
        ) : (
          <span className="whitespace-pre-wrap">{part.text}</span>
        )}
      </div>
    </li>
  );
});
Row.displayName = "TranscriptRow";

/** A transcript that follows the player. All of its text is untrusted, and all of it is text. */
export const TranscriptView = ({ parts, timeMs, fromMs, toMs, onSeek, hasMore, loadingMore, onLoadMore }: TranscriptViewProps) => {
  const scroller = useRef<HTMLOListElement>(null);
  const [follow, setFollow] = useState(true);
  const active = activePart(parts, timeMs);
  const inRange = useMemo(() => new Set(fromMs === null ? [] : partsInRange(parts, fromMs, toMs)), [fromMs, parts, toMs]);

  useEffect(() => {
    const list = scroller.current;
    if (!follow || list === null || active === null) return;
    const row = list.children[active];
    if (row instanceof HTMLElement) list.scrollTop = Math.max(0, row.offsetTop - list.offsetTop - list.clientHeight / 3);
  }, [active, follow]);

  return (
    <section aria-label="Transcript" className="flex flex-col gap-2">
      <div className="flex items-center justify-between gap-2">
        <h2 className="font-medium">Transcript</h2>
        <Button aria-pressed={follow} onClick={() => setFollow((f) => !f)}>Follow playback</Button>
      </div>
      <ol ref={scroller} tabIndex={0} aria-label="Transcript text" className="flex h-96 flex-col gap-0.5 overflow-auto rounded-md border border-border p-1">
        {parts.map((part, index) => (
          <Row key={part.start} part={part} active={index === active} inRange={inRange.has(index)} timeMs={index === active ? timeMs : null} onSeek={onSeek} />
        ))}
      </ol>
      {hasMore && <Button disabled={loadingMore} onClick={onLoadMore}>Load more</Button>}
    </section>
  );
};
