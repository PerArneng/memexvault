import { describe, expect, it } from "vitest";
import { conflictCount, mergedBody, threeWayMerge } from "./merge";

const body = (...blocks: readonly string[]) => `${blocks.join("\n\n")}\n`;

describe("threeWayMerge", () => {
  it("takes each side's change when they don't touch", () => {
    const hunks = threeWayMerge(body("a", "b", "c"), body("A", "b", "c"), body("a", "b", "C"));
    expect(conflictCount(hunks)).toBe(0);
    expect(mergedBody(hunks, {})).toBe(body("A", "b", "C"));
  });

  it("marks the same block changed two ways as a conflict, keeping both sides and the base", () => {
    const hunks = threeWayMerge(body("a", "b"), body("a", "mine"), body("a", "theirs"));
    expect(hunks).toEqual([
      { kind: "agreed", blocks: ["a"] },
      { kind: "conflict", id: 0, base: ["b"], mine: ["mine"], theirs: ["theirs"] },
    ]);
  });

  it("agrees when both made the same change", () => {
    const hunks = threeWayMerge(body("a"), body("same"), body("same"));
    expect(conflictCount(hunks)).toBe(0);
    expect(mergedBody(hunks, {})).toBe(body("same"));
  });

  it("gives nothing to save until every conflict has a choice", () => {
    const hunks = threeWayMerge(body("x", "keep", "y"), body("x1", "keep", "y1"), body("x2", "keep", "y2"));
    expect(conflictCount(hunks)).toBe(2);
    expect(mergedBody(hunks, {})).toBeNull();
    expect(mergedBody(hunks, { 0: "mine" })).toBeNull();
    expect(mergedBody(hunks, { 0: "mine", 1: "theirs" })).toBe(body("x1", "keep", "y2"));
    expect(mergedBody(hunks, { 0: "both", 1: "both" })).toBe(body("x1", "x2", "keep", "y1", "y2"));
  });

  it("handles additions at either end, and a delete against an untouched block", () => {
    expect(mergedBody(threeWayMerge(body("a"), body("new", "a"), body("a", "end")), {})).toBe(body("new", "a", "end"));
    expect(mergedBody(threeWayMerge(body("a", "b"), body("a"), body("a", "b")), {})).toBe(body("a"));
  });

  it("conflicts when one deletes what the other changed", () => {
    const hunks = threeWayMerge(body("a", "b"), body("a"), body("a", "changed"));
    expect(hunks[1]).toEqual({ kind: "conflict", id: 0, base: ["b"], mine: [], theirs: ["changed"] });
    expect(mergedBody(hunks, { 0: "mine" })).toBe(body("a"));
  });

  it("works with no base, as when the base revision can't be read: differing text conflicts", () => {
    const hunks = threeWayMerge("", body("mine"), body("theirs"));
    expect(conflictCount(hunks)).toBe(1);
    expect(mergedBody(hunks, { 0: "both" })).toBe(body("mine", "theirs"));
  });

  it("keeps a code fence whole, with its blank lines", () => {
    const fence = "```\nx\n\ny\n```";
    expect(mergedBody(threeWayMerge(body(fence), body(fence, "m"), body(fence)), {})).toBe(body(fence, "m"));
  });

  it("gives an empty body for all-empty sides", () => {
    expect(mergedBody(threeWayMerge("", "", ""), {})).toBe("");
  });

  it("copes with repeated blocks", () => {
    const hunks = threeWayMerge(body("x", "x", "x"), body("x", "x"), body("x", "x", "x", "t"));
    // One side dropped the last block and the other added after it, which is a real conflict, and nothing is lost either way.
    expect(conflictCount(hunks)).toBe(1);
    expect(mergedBody(hunks, { 0: "mine" })).toBe(body("x", "x"));
    expect(mergedBody(hunks, { 0: "theirs" })).toBe(body("x", "x", "x", "t"));
    expect(mergedBody(threeWayMerge(body("x", "x"), body("m", "x", "x"), body("x", "x", "t")), {})).toBe(body("m", "x", "x", "t"));
  });

  it("handles a page at the server's limit of 2,000 blocks", () => {
    const base = Array.from({ length: 2000 }, (_, i) => `block ${String(i)}`);
    const theirs = base.map((b, i) => (i === 1000 ? "theirs" : b));
    const mine = base.map((b, i) => (i % 500 === 7 ? `mine ${String(i)}` : b));
    const hunks = threeWayMerge(body(...base), body(...mine), body(...theirs));
    expect(conflictCount(hunks)).toBe(0);
    expect(mergedBody(hunks, {})).toContain("theirs");
    expect(mergedBody(hunks, {})).toContain("mine 507");
  });
});
