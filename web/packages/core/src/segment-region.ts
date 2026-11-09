import type { AnchorRect, PageWord } from "@memex/types";

const utf8Length = (text: string): number => {
  let bytes = 0;
  for (const char of text) {
    const code = char.codePointAt(0) ?? 0;
    bytes += code < 0x80 ? 1 : code < 0x800 ? 2 : code < 0x10000 ? 3 : 4;
  }
  return bytes;
};

const sameLine = (a: AnchorRect, word: PageWord): boolean => {
  if (a.page !== word.page) return false;
  const middle = (word.rect.y0 + word.rect.y1) / 2;
  return middle >= a.y0 && middle <= a.y1;
};

/**
 * Where a stretch of a rendition's text sits on its pages: one rectangle a line, as an anchor has. A segment gives byte offsets, so this
 * walks the canonical text, which holds every word in the word table's order with separators between. Null when the two don't agree, and
 * the screen then says so rather than guessing.
 */
export const segmentRegion = (canonicalText: string, words: readonly PageWord[], start: number, end: number): readonly AnchorRect[] | null => {
  const rects: AnchorRect[] = [];
  let at = 0;
  let bytes = 0;
  for (const word of words) {
    if (word.text === "") continue;
    const found = canonicalText.indexOf(word.text, at);
    if (found < 0) return null;
    const wordStart = bytes + utf8Length(canonicalText.slice(at, found));
    const wordEnd = wordStart + utf8Length(word.text);
    if (wordStart >= end) break;
    if (wordEnd > start) {
      const last = rects[rects.length - 1];
      if (last !== undefined && sameLine(last, word)) {
        rects[rects.length - 1] = { page: last.page, x0: Math.min(last.x0, word.rect.x0), y0: Math.min(last.y0, word.rect.y0), x1: Math.max(last.x1, word.rect.x1), y1: Math.max(last.y1, word.rect.y1) };
      } else {
        rects.push({ page: word.page, ...word.rect });
      }
    }
    at = found + word.text.length;
    bytes = wordEnd;
  }
  return rects.length === 0 ? null : rects;
};
