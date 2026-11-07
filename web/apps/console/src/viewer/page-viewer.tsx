import { clampPage, dpiFor, overlayBoxes, pageAtMiddle, pageSizeAt, stackPages, visiblePages, zoomIn, zoomOut, ZOOM_LEVELS } from "@memex/core";
import type { AnchorRect, PageFact, PageWord } from "@memex/types";
import { Button, Skeleton } from "@memex/ui";
import { ChevronLeft, ChevronRight, Flag, ZoomIn, ZoomOut } from "lucide-react";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { ChangeEvent, ReactNode } from "react";
import { WordLayer } from "./word-layer";

/** The space between pages, and above the first highlight when jumping to it, in CSS pixels. */
const PAGE_GAP = 16;
const HIGHLIGHT_MARGIN = 96;
/** Rendering a page costs the server real work, so a page scrolled straight past isn't asked for. */
const SCROLL_QUIET_MS = 120;
const THUMB_WIDTH = 96;
const THUMB_GAP = 28;

export interface PageViewerProps {
  readonly pages: readonly PageFact[];
  readonly allowedDpi: readonly number[];
  readonly imageUrl: (page: number, dpi: number) => string;
  readonly thumbnailUrl: (page: number) => string;
  /** Says why an image wouldn't load, since the browser won't. */
  readonly explainFailure: (page: number, dpi: number) => Promise<string>;
  readonly highlights: readonly AnchorRect[];
  /** Moving this scrolls to it. */
  readonly page: number;
  readonly zoom: number;
  readonly onPageChange: (page: number) => void;
  readonly onZoomChange: (zoom: number) => void;
  /** Pages to mark in the thumbnails, with why. */
  readonly marks?: ReadonlyMap<number, string>;
  /** Null until words are asked for. */
  readonly wordsOnPage?: ((page: number) => readonly PageWord[]) | null;
  /** The direction text runs on a page. */
  readonly textAngle?: (page: PageFact) => number;
  readonly toolbar?: ReactNode;
}

const scrollIn = (element: HTMLElement | null, top: number) => {
  if (element !== null) element.scrollTop = top;
};

const PageFailure = ({ number, explain, onRetry }: { readonly number: number; readonly explain: () => Promise<string>; readonly onRetry: () => void }) => {
  const [why, setWhy] = useState<string | null>(null);
  useEffect(() => {
    let current = true;
    void explain().then((text) => {
      if (current) setWhy(text);
    });
    return () => {
      current = false;
    };
  }, [explain]);
  return (
    <div role="alert" data-page-failure={number} className="absolute inset-0 flex flex-col items-center justify-center gap-2 border border-danger bg-danger-surface p-4 text-center">
      <p className="font-medium text-danger">Couldn't show page {number}.</p>
      <p className="text-sm text-text-muted">{why ?? "Asking the server why."}</p>
      <Button onClick={onRetry}>Retry</Button>
    </div>
  );
};

interface PageCellProps {
  readonly fact: PageFact;
  readonly top: number;
  readonly left: number;
  readonly zoom: number;
  readonly dpi: number;
  readonly url: string;
  readonly settled: boolean;
  readonly highlights: readonly AnchorRect[];
  readonly words: readonly PageWord[] | null;
  readonly angle: number;
  readonly explain: (page: number, dpi: number) => Promise<string>;
  readonly onSettled: () => void;
}

/** One page: its image once the scrolling has stopped, its highlights, and its words if they're wanted. */
const PageCell = ({ fact, top, left, zoom, dpi, url, settled, highlights, words, angle, explain, onSettled }: PageCellProps) => {
  const { width, height } = pageSizeAt(fact, zoom);
  const [state, setState] = useState<"waiting" | "loaded" | "failed">("waiting");
  const [attempt, setAttempt] = useState(0);
  // It stays asked for if scrolling starts again, as the request has gone already.
  const [asked, setAsked] = useState(settled);
  useEffect(() => {
    if (settled) setAsked(true);
  }, [settled]);
  useEffect(() => setState("waiting"), [url]);
  const explainThis = useCallback(() => explain(fact.index, dpi), [explain, fact.index, dpi]);
  const boxes = overlayBoxes(highlights, fact.index, fact, zoom);
  const number = fact.index + 1;
  return (
    <div data-page-index={fact.index} data-page-state={state} className="absolute bg-neutral-surface shadow-overlay" style={{ top, left, width, height }}>
      {state === "waiting" && <Skeleton className="absolute inset-0" />}
      {state === "failed" ? (
        <PageFailure
          number={number}
          explain={explainThis}
          onRetry={() => {
            setAttempt((a) => a + 1);
            setState("waiting");
          }}
        />
      ) : (
        asked && (
          <img
            key={`${url}#${String(attempt)}`}
            src={url}
            alt={`Page ${String(number)}`}
            draggable={false}
            className="absolute inset-0 select-none"
            style={{ width, height }}
            onLoad={() => {
              setState("loaded");
              onSettled();
            }}
            onError={() => {
              setState("failed");
              onSettled();
            }}
          />
        )
      )}
      {state !== "failed" && (
        <>
          {boxes.map((box, index) => (
            <span
              key={index}
              data-highlight={fact.index}
              aria-hidden={true}
              className="pointer-events-none absolute bg-warning/40"
              style={{ left: box.left, top: box.top, width: box.width, height: box.height }}
            />
          ))}
          {boxes.length > 0 && <span className="sr-only">Page {number} has the quoted text highlighted.</span>}
          {words !== null && <WordLayer words={words} angle={angle} zoom={zoom} />}
        </>
      )}
    </div>
  );
};

interface RailProps {
  readonly pages: readonly PageFact[];
  readonly current: number;
  readonly url: (page: number) => string;
  readonly marks: ReadonlyMap<number, string>;
  readonly onPick: (page: number) => void;
}

/** Thumbnails, of which only those in view exist, like the pages. */
const ThumbnailRail = ({ pages, current, url, marks, onPick }: RailProps) => {
  const scroller = useRef<HTMLDivElement>(null);
  const [view, setView] = useState({ top: 0, height: 0 });
  const heights = useMemo(() => pages.map((p) => (p.width > 0 ? (p.height / p.width) * THUMB_WIDTH : THUMB_WIDTH)), [pages]);
  const stack = useMemo(() => stackPages(heights, THUMB_GAP), [heights]);
  const measure = useCallback(() => {
    const element = scroller.current;
    if (element !== null) setView({ top: element.scrollTop, height: element.clientHeight });
  }, []);
  useLayoutEffect(measure, [measure]);
  useEffect(() => {
    const element = scroller.current;
    if (element === null) return;
    const top = stack.tops[current] ?? 0;
    const bottom = top + (stack.heights[current] ?? 0) + THUMB_GAP;
    if (top < element.scrollTop) element.scrollTop = top;
    else if (bottom > element.scrollTop + element.clientHeight) element.scrollTop = bottom - element.clientHeight;
    measure();
  }, [current, measure, stack]);
  const { first, last } = visiblePages(stack, view.top, view.height);
  const shown = [];
  for (let index = first; index <= last; index += 1) {
    const mark = marks.get(index);
    shown.push(
      <button
        key={index}
        type="button"
        aria-label={`Go to page ${String(index + 1)}${mark === undefined ? "" : `. Flagged: ${mark}`}`}
        aria-current={index === current ? "page" : undefined}
        title={mark}
        onClick={() => onPick(index)}
        className="absolute left-2 flex flex-col items-center gap-0.5 text-xs text-text-muted"
        style={{ top: stack.tops[index] ?? 0, width: THUMB_WIDTH }}
      >
        <img data-thumbnail={index} src={url(index)} alt="" draggable={false} className={`block bg-neutral-surface ${index === current ? "outline-2 outline-accent" : ""}`} style={{ width: THUMB_WIDTH, height: heights[index] ?? THUMB_WIDTH }} />
        <span className="inline-flex items-center gap-1">
          {mark !== undefined && <Flag size={10} aria-hidden={true} className="text-warning" />}
          {index + 1}
        </span>
      </button>,
    );
  }
  return (
    <div ref={scroller} onScroll={measure} role="group" aria-label="Page thumbnails" className="relative shrink-0 overflow-auto border-r border-border" style={{ width: THUMB_WIDTH + 28 }}>
      <div style={{ height: stack.total + THUMB_GAP }}>{shown}</div>
    </div>
  );
};

/**
 * Server-rendered pages in a scrolling column, with only the visible ones in the DOM, so a 400-page document fetches what's on screen and
 * nothing else. It knows nothing of sources, claims or URLs, so any screen can use it: everything comes in through props.
 */
export const PageViewer = ({ pages, allowedDpi, imageUrl, thumbnailUrl, explainFailure, highlights, page, zoom, onPageChange, onZoomChange, marks, wordsOnPage = null, textAngle, toolbar }: PageViewerProps) => {
  const scroller = useRef<HTMLDivElement>(null);
  const [view, setView] = useState({ top: 0, height: 0 });
  const [settled, setSettled] = useState(true);
  const [firstImageDone, setFirstImageDone] = useState(false);
  const quiet = useRef<ReturnType<typeof setTimeout> | null>(null);
  // Set while the viewer moves itself, so that isn't mistaken for the reader scrolling.
  const reported = useRef(page);

  const sizes = useMemo(() => pages.map((p) => pageSizeAt(p, zoom)), [pages, zoom]);
  const stack = useMemo(() => stackPages(sizes.map((s) => s.height), PAGE_GAP), [sizes]);
  const widest = useMemo(() => sizes.reduce((most, s) => Math.max(most, s.width), 0), [sizes]);
  const dpi = dpiFor(zoom, typeof window === "undefined" ? 1 : window.devicePixelRatio, allowedDpi);

  const targetOf = useCallback(
    (index: number) => {
      const fact = pages[index];
      const pageTop = stack.tops[index] ?? 0;
      if (fact === undefined) return pageTop;
      const box = overlayBoxes(highlights, index, fact, zoom)[0];
      return box === undefined ? pageTop : Math.max(0, pageTop + box.top - HIGHLIGHT_MARGIN);
    },
    [highlights, pages, stack, zoom],
  );

  const measure = useCallback(() => {
    const element = scroller.current;
    if (element !== null) setView({ top: element.scrollTop, height: element.clientHeight });
  }, []);

  // Before the first paint, so a link to page 200 never asks for page 1.
  const placed = useRef(false);
  useLayoutEffect(() => {
    const element = scroller.current;
    if (element === null) return;
    if (!placed.current) {
      placed.current = true;
      scrollIn(element, targetOf(clampPage(page, pages.length)));
    }
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, [measure, page, pages.length, targetOf]);

  // Near a document's end the column can't scroll far enough, and what's above the viewer can push it off the screen, so the first highlight is brought into view once it's drawn.
  const shownHighlight = useRef(false);
  const hasHighlights = highlights.length > 0;
  useLayoutEffect(() => {
    if (shownHighlight.current || !hasHighlights) return;
    const first = scroller.current?.querySelector("[data-highlight]");
    if (first !== null && first !== undefined) {
      shownHighlight.current = true;
      first.scrollIntoView({ block: "center" });
    }
  });

  // A link inside the same document moves here without remounting. The reader's own scrolling reports the same number, so it does nothing.
  useEffect(() => {
    if (page !== reported.current) {
      reported.current = page;
      scrollIn(scroller.current, targetOf(clampPage(page, pages.length)));
      measure();
    }
  }, [measure, page, pages.length, targetOf]);

  // Zooming keeps the same part of the document on screen.
  const lastTotal = useRef(stack.total);
  useLayoutEffect(() => {
    const element = scroller.current;
    if (element !== null && lastTotal.current > 0 && lastTotal.current !== stack.total) {
      element.scrollTop = element.scrollTop * (stack.total / lastTotal.current);
      measure();
    }
    lastTotal.current = stack.total;
  }, [measure, stack.total]);

  useEffect(
    () => () => {
      if (quiet.current !== null) clearTimeout(quiet.current);
    },
    [],
  );

  const onScroll = () => {
    measure();
    setSettled(false);
    if (quiet.current !== null) clearTimeout(quiet.current);
    quiet.current = setTimeout(() => {
      setSettled(true);
      const element = scroller.current;
      if (element === null) return;
      const middle = pageAtMiddle(stack, element.scrollTop, element.clientHeight);
      if (middle !== reported.current) {
        reported.current = middle;
        onPageChange(middle);
      }
    }, SCROLL_QUIET_MS);
  };

  const current = clampPage(pageAtMiddle(stack, view.top, view.height), pages.length);
  const go = (index: number) => {
    const target = clampPage(index, pages.length);
    reported.current = target;
    scrollIn(scroller.current, stack.tops[target] ?? 0);
    measure();
    onPageChange(target);
  };

  const { first, last } = visiblePages(stack, view.top, view.height);
  const visible = [];
  for (let index = first; index <= last; index += 1) {
    const fact = pages[index];
    const size = sizes[index];
    if (fact !== undefined && size !== undefined && dpi !== null) {
      visible.push(
        <PageCell
          key={index}
          fact={fact}
          top={stack.tops[index] ?? 0}
          left={(widest - size.width) / 2}
          zoom={zoom}
          dpi={dpi}
          url={imageUrl(index, dpi)}
          settled={settled}
          highlights={highlights}
          words={wordsOnPage === null ? null : wordsOnPage(index)}
          angle={textAngle === undefined ? fact.rotation : textAngle(fact)}
          explain={explainFailure}
          onSettled={() => setFirstImageDone(true)}
        />,
      );
    }
  }

  return (
    <section aria-label="Document" className="flex flex-col gap-2">
      <div role="toolbar" aria-label="Page controls" className="flex flex-wrap items-center gap-2">
        <Button aria-label="Previous page" disabled={current === 0} onClick={() => go(current - 1)}><ChevronLeft size={14} aria-hidden={true} /></Button>
        <label className="inline-flex items-center gap-1 text-sm text-text-muted">
          Page
          <input
            key={current}
            aria-label="Page number"
            inputMode="numeric"
            defaultValue={current + 1}
            onKeyDown={(e) => {
              if (e.key === "Enter") go(Number.parseInt(e.currentTarget.value, 10) - 1 || 0);
            }}
            className="h-8 w-14 rounded-md border border-border-strong bg-surface-raised px-2 text-right text-sm text-text"
          />
          of {pages.length}
        </label>
        <Button aria-label="Next page" disabled={current >= pages.length - 1} onClick={() => go(current + 1)}><ChevronRight size={14} aria-hidden={true} /></Button>
        <span className="mx-1 h-5 w-px bg-border" aria-hidden={true} />
        <Button aria-label="Zoom out" disabled={zoom <= (ZOOM_LEVELS[0] ?? zoom)} onClick={() => onZoomChange(zoomOut(zoom))}><ZoomOut size={14} aria-hidden={true} /></Button>
        <select
          aria-label="Zoom"
          value={zoom}
          onChange={(e: ChangeEvent<HTMLSelectElement>) => onZoomChange(Number(e.target.value))}
          className="h-8 rounded-md border border-border-strong bg-surface-raised px-1 text-sm text-text"
        >
          {ZOOM_LEVELS.map((z) => <option key={z} value={z}>{z}%</option>)}
        </select>
        <Button aria-label="Zoom in" disabled={zoom >= (ZOOM_LEVELS[ZOOM_LEVELS.length - 1] ?? zoom)} onClick={() => onZoomChange(zoomIn(zoom))}><ZoomIn size={14} aria-hidden={true} /></Button>
        {toolbar}
      </div>
      <div className="flex h-[calc(100vh-15rem)] min-h-96 rounded-md border border-border">
        {firstImageDone && <ThumbnailRail pages={pages} current={current} url={thumbnailUrl} marks={marks ?? new Map()} onPick={go} />}
        <div ref={scroller} onScroll={onScroll} tabIndex={0} role="group" aria-label="Pages" className="relative min-w-0 flex-1 overflow-auto bg-surface p-0">
          <div className="relative mx-auto" style={{ width: widest, height: stack.total }}>{visible}</div>
        </div>
      </div>
    </section>
  );
};
