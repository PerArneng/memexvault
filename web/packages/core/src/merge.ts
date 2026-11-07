/** Three-way merge of Markdown by block, which is how the server diffs too. It only says what agrees, and saves nothing. */
import { blocksOf } from "./wikitext";

/** A body's blocks, so a merge cuts where the server's block diff does. */
const blockTexts = (body: string): readonly string[] => {
  const text = body.replace(/\r\n?/g, "\n");
  return blocksOf(text).map((b) => text.slice(b.start, b.end).trimEnd());
};

/** Pairs of equal entries, as a longest common subsequence. The cost is the square of what's left after common ends, within the server's 2,000-block limit. */
const matchesBetween = (a: readonly string[], b: readonly string[]): readonly (readonly [number, number])[] => {
  let start = 0;
  while (start < a.length && start < b.length && a[start] === b[start]) start += 1;
  let endA = a.length;
  let endB = b.length;
  while (endA > start && endB > start && a[endA - 1] === b[endB - 1]) {
    endA -= 1;
    endB -= 1;
  }
  const rows = endA - start;
  const cols = endB - start;
  const width = cols + 1;
  const lengths = new Uint32Array((rows + 1) * width);
  for (let i = rows - 1; i >= 0; i -= 1) {
    for (let j = cols - 1; j >= 0; j -= 1) {
      lengths[i * width + j] = a[start + i] === b[start + j] ? (lengths[(i + 1) * width + j + 1] ?? 0) + 1 : Math.max(lengths[(i + 1) * width + j] ?? 0, lengths[i * width + j + 1] ?? 0);
    }
  }
  const pairs: (readonly [number, number])[] = [];
  for (let i = 0; i < start; i += 1) pairs.push([i, i]);
  let i = 0;
  let j = 0;
  while (i < rows && j < cols) {
    if (a[start + i] === b[start + j]) {
      pairs.push([start + i, start + j]);
      i += 1;
      j += 1;
    } else if ((lengths[(i + 1) * width + j] ?? 0) >= (lengths[i * width + j + 1] ?? 0)) i += 1;
    else j += 1;
  }
  for (let k = 0; k < a.length - endA; k += 1) pairs.push([endA + k, endB + k]);
  return pairs;
};

export type MergeHunk =
  | { readonly kind: "agreed"; readonly blocks: readonly string[] }
  | { readonly kind: "conflict"; readonly id: number; readonly base: readonly string[]; readonly mine: readonly string[]; readonly theirs: readonly string[] };

export type MergeChoice = "mine" | "theirs" | "both";

const same = (a: readonly string[], b: readonly string[]): boolean => a.length === b.length && a.every((x, i) => x === b[i]);

/** The standard diff3 walk: stable regions are base blocks both sides kept, and each stretch between them is taken from whichever side changed it. */
export const threeWayMerge = (baseBody: string, mineBody: string, theirsBody: string): readonly MergeHunk[] => {
  const base = blockTexts(baseBody);
  const mine = blockTexts(mineBody);
  const theirs = blockTexts(theirsBody);
  const inMine = new Map(matchesBetween(base, mine));
  const inTheirs = new Map(matchesBetween(base, theirs));
  const hunks: MergeHunk[] = [];
  let conflicts = 0;
  const agreed = (blocks: readonly string[]) => {
    if (blocks.length === 0) return;
    const last = hunks[hunks.length - 1];
    if (last?.kind === "agreed") hunks[hunks.length - 1] = { kind: "agreed", blocks: [...last.blocks, ...blocks] };
    else hunks.push({ kind: "agreed", blocks });
  };
  const between = (b: readonly string[], m: readonly string[], t: readonly string[]) => {
    if (same(m, t) || same(b, t)) agreed(m);
    else if (same(b, m)) agreed(t);
    else if (b.length > 1 && m.length === b.length && t.length === b.length) {
      // Edited in place on both sides, which the server's block matching also treats as the same blocks, so each pairs off by position.
      b.forEach((block, i) => between([block], m.slice(i, i + 1), t.slice(i, i + 1)));
    } else {
      hunks.push({ kind: "conflict", id: conflicts, base: b, mine: m, theirs: t });
      conflicts += 1;
    }
  };
  let bi = 0;
  let mi = 0;
  let ti = 0;
  for (let i = 0; i < base.length; i += 1) {
    const m = inMine.get(i);
    const t = inTheirs.get(i);
    if (m === undefined || t === undefined || m < mi || t < ti) continue;
    between(base.slice(bi, i), mine.slice(mi, m), theirs.slice(ti, t));
    agreed(base.slice(i, i + 1));
    bi = i + 1;
    mi = m + 1;
    ti = t + 1;
  }
  between(base.slice(bi), mine.slice(mi), theirs.slice(ti));
  return hunks;
};

export const conflictCount = (hunks: readonly MergeHunk[]): number => hunks.filter((h) => h.kind === "conflict").length;

/** The body the choices make. A conflict with no choice yet leaves null, as nothing should be saved until a person has decided. */
export const mergedBody = (hunks: readonly MergeHunk[], choices: Readonly<Record<number, MergeChoice>>): string | null => {
  const blocks: string[] = [];
  for (const hunk of hunks) {
    if (hunk.kind === "agreed") blocks.push(...hunk.blocks);
    else {
      const choice = choices[hunk.id];
      if (choice === undefined) return null;
      if (choice !== "theirs") blocks.push(...hunk.mine);
      if (choice !== "mine") blocks.push(...hunk.theirs);
    }
  }
  return blocks.length === 0 ? "" : `${blocks.join("\n\n")}\n`;
};
