import type { PageWord } from "@memex/types";
import { describe, expect, it } from "vitest";
import { segmentRegion } from "./segment-region";

const word = (text: string, page: number, x0: number, y0: number): PageWord => ({ text, page, rect: { x0, y0, x1: x0 + 100, y1: y0 + 20 }, quad: null });
const bytes = (text: string): number => {
  let n = 0;
  for (const c of text) {
    const code = c.codePointAt(0) ?? 0;
    n += code < 0x80 ? 1 : code < 0x800 ? 2 : code < 0x10000 ? 3 : 4;
  }
  return n;
};

const WORDS = [word("Åsa", 0, 0, 0), word("bor", 0, 200, 0), word("här", 0, 0, 100), word("Straße", 1, 0, 0), word("😀", 1, 200, 0), word("niño", 1, 400, 0)];
const TEXT = "Åsa bor\nhär\n\nStraße 😀 niño";

const rangeOf = (needle: string): readonly [number, number] => {
  const at = TEXT.indexOf(needle);
  return [bytes(TEXT.slice(0, at)), bytes(TEXT.slice(0, at)) + bytes(needle)];
};

describe("segmentRegion", () => {
  it("joins the words of a line into one rectangle", () => {
    const [start, end] = rangeOf("Åsa bor");
    expect(segmentRegion(TEXT, WORDS, start, end)).toEqual([{ page: 0, x0: 0, y0: 0, x1: 300, y1: 20 }]);
  });

  it("gives each line and each page its own", () => {
    const [start, end] = rangeOf("bor\nhär\n\nStraße");
    expect(segmentRegion(TEXT, WORDS, start, end)).toEqual([
      { page: 0, x0: 200, y0: 0, x1: 300, y1: 20 },
      { page: 0, x0: 0, y0: 100, x1: 100, y1: 120 },
      { page: 1, x0: 0, y0: 0, x1: 100, y1: 20 },
    ]);
  });

  it("counts bytes, as the offsets do, past text of every width", () => {
    const [start, end] = rangeOf("niño");
    expect(segmentRegion(TEXT, WORDS, start, end)).toEqual([{ page: 1, x0: 400, y0: 0, x1: 500, y1: 20 }]);
  });

  it("takes a word the range only touches part of", () => {
    const [start] = rangeOf("här");
    expect(segmentRegion(TEXT, WORDS, start + 1, start + 2)).toEqual([{ page: 0, x0: 0, y0: 100, x1: 100, y1: 120 }]);
  });

  it("has no answer for a range with no words in it", () => {
    expect(segmentRegion(TEXT, WORDS, 10_000, 10_010)).toBeNull();
    expect(segmentRegion(TEXT, WORDS, 0, 0)).toBeNull();
    expect(segmentRegion("", [], 0, 10)).toBeNull();
  });

  it("refuses to guess when the text and the table disagree", () => {
    expect(segmentRegion("something else entirely", WORDS, 0, 10)).toBeNull();
  });

  it("keeps two columns apart when they share a height, as the table's order isn't the page's", () => {
    const columns = [word("left", 0, 0, 0), word("one", 0, 0, 100), word("right", 0, 500, 0)];
    const text = "left\none\n\nright";
    expect(segmentRegion(text, columns, bytes("left\none\n\n"), bytes(text))).toEqual([{ page: 0, x0: 500, y0: 0, x1: 600, y1: 20 }]);
  });
});
