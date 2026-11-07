/** Where the pages of a long document sit in a scrolled column, and which of them can be seen, so only those are fetched. */
export interface PageStack {
  /** Where each page starts. */
  readonly tops: readonly number[];
  readonly heights: readonly number[];
  readonly total: number;
}

export const stackPages = (heights: readonly number[], gap: number): PageStack => {
  const tops: number[] = [];
  let at = 0;
  for (const height of heights) {
    tops.push(at);
    at += height + gap;
  }
  return { tops, heights, total: heights.length === 0 ? 0 : at - gap };
};

export interface PageRange {
  readonly first: number;
  /** Inclusive. Less than `first` when nothing is visible. */
  readonly last: number;
}

const NOTHING: PageRange = { first: 0, last: -1 };

/** The first page whose bottom edge is below `y`, by bisection, as a stack can hold thousands. */
const pageReaching = ({ tops, heights }: PageStack, y: number): number => {
  let low = 0;
  let high = tops.length - 1;
  while (low < high) {
    const middle = Math.floor((low + high) / 2);
    if ((tops[middle] ?? 0) + (heights[middle] ?? 0) > y) high = middle;
    else low = middle + 1;
  }
  return low;
};

/** The pages that overlap the part of the column on screen. A page that only touches its edge isn't one. */
export const visiblePages = (stack: PageStack, scrollTop: number, viewport: number): PageRange => {
  if (stack.tops.length === 0 || viewport <= 0) return NOTHING;
  const end = scrollTop + viewport;
  const first = pageReaching(stack, scrollTop);
  let last = first;
  while (last + 1 < stack.tops.length && (stack.tops[last + 1] ?? end) < end) last += 1;
  return (stack.tops[first] ?? end) < end ? { first, last } : NOTHING;
};

/** The page to call current: the one at the middle of the screen, or the nearest before it in a gap. */
export const pageAtMiddle = (stack: PageStack, scrollTop: number, viewport: number): number => {
  if (stack.tops.length === 0) return 0;
  const middle = scrollTop + viewport / 2;
  const reached = pageReaching(stack, middle);
  return (stack.tops[reached] ?? 0) > middle ? Math.max(0, reached - 1) : reached;
};

export const clampPage = (page: number, count: number): number => Math.min(Math.max(page, 0), Math.max(count - 1, 0));
