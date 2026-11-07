import { describe, expect, it } from "vitest";
import { clampPage, pageAtMiddle, stackPages, visiblePages } from "./page-stack";

const FOUR_HUNDRED = stackPages(Array.from({ length: 400 }, () => 842), 16);

describe("a stack of pages", () => {
  it("lays pages out with gaps, and has no gap after the last", () => {
    const stack = stackPages([100, 200, 50], 10);
    expect(stack.tops).toEqual([0, 110, 320]);
    expect(stack.total).toBe(370);
  });

  it("is empty with no pages", () => {
    expect(stackPages([], 10)).toEqual({ tops: [], heights: [], total: 0 });
    expect(visiblePages(stackPages([], 10), 0, 500)).toEqual({ first: 0, last: -1 });
    expect(pageAtMiddle(stackPages([], 10), 0, 500)).toBe(0);
  });
});

describe("which pages are visible", () => {
  it("is only the first page or two when a long document opens", () => {
    expect(visiblePages(FOUR_HUNDRED, 0, 800)).toEqual({ first: 0, last: 0 });
    expect(visiblePages(FOUR_HUNDRED, 0, 900)).toEqual({ first: 0, last: 1 });
  });

  it("has no page beyond those on screen, wherever it's scrolled to", () => {
    for (const scrollTop of [0, 5_000, 100_000, FOUR_HUNDRED.total - 800]) {
      const { first, last } = visiblePages(FOUR_HUNDRED, scrollTop, 800);
      expect(last - first).toBeLessThanOrEqual(1);
      for (let page = first; page <= last; page += 1) {
        const top = FOUR_HUNDRED.tops[page] ?? 0;
        expect(top).toBeLessThan(scrollTop + 800);
        expect(top + 842).toBeGreaterThan(scrollTop);
      }
    }
  });

  it("doesn't count a page that only touches the edge", () => {
    const stack = stackPages([100, 100], 0);
    expect(visiblePages(stack, 0, 100)).toEqual({ first: 0, last: 0 });
    expect(visiblePages(stack, 100, 100)).toEqual({ first: 1, last: 1 });
  });

  it("is the last page when scrolled past the end, and nothing with no screen", () => {
    expect(visiblePages(stackPages([100, 100], 0), 150, 10)).toEqual({ first: 1, last: 1 });
    expect(visiblePages(FOUR_HUNDRED, 0, 0)).toEqual({ first: 0, last: -1 });
  });

  it("handles pages of different sizes", () => {
    const stack = stackPages([100, 900, 50, 400], 0);
    expect(visiblePages(stack, 950, 100)).toEqual({ first: 1, last: 2 });
    expect(visiblePages(stack, 1060, 10)).toEqual({ first: 3, last: 3 });
  });

  it("stays in a gap when only the gap is on screen", () => {
    expect(visiblePages(stackPages([100, 100], 50), 105, 10)).toEqual({ first: 0, last: -1 });
  });
});

describe("the current page", () => {
  it("is the one at the middle of the screen", () => {
    expect(pageAtMiddle(FOUR_HUNDRED, 0, 800)).toBe(0);
    expect(pageAtMiddle(FOUR_HUNDRED, 858 * 10, 800)).toBe(10);
  });

  it("in a gap is the page before it", () => {
    expect(pageAtMiddle(stackPages([100, 100], 50), 50, 100)).toBe(0);
  });
});

describe("a page number from a URL", () => {
  it("is held to the document", () => {
    expect(clampPage(-1, 10)).toBe(0);
    expect(clampPage(10, 10)).toBe(9);
    expect(clampPage(5, 10)).toBe(5);
    expect(clampPage(5, 0)).toBe(0);
  });
});
