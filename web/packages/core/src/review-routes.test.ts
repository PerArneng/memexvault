import { vaultId } from "@memex/types";
import { describe, expect, it } from "vitest";
import { NO_FILTER, parseReviewRoute, reviewPath } from "./review-routes";

const vault = vaultId("v");
const none = () => null;
const query = (values: Readonly<Record<string, string>>) => (name: string) => values[name] ?? null;

describe("review routes", () => {
  it("reads the queue, an item, and each tab", () => {
    expect(parseReviewRoute([], none)).toEqual({ kind: "queue", filter: NO_FILTER, item: null });
    expect(parseReviewRoute(["cs1"], none)).toEqual({ kind: "queue", filter: NO_FILTER, item: "cs1" });
    expect(parseReviewRoute(["-", "changesets"], none)).toEqual({ kind: "changesets", state: "" });
    expect(parseReviewRoute(["-", "changesets", "cs1"], none)).toEqual({ kind: "changeset", id: "cs1" });
    expect(parseReviewRoute(["-", "contradictions"], none)).toEqual({ kind: "contradictions" });
    expect(parseReviewRoute(["-", "auto-commit"], none)).toEqual({ kind: "auto_commit" });
  });
  it("reads filters from the URL, and ignores a verdict or state it doesn't know", () => {
    expect(parseReviewRoute([], query({ type: "claim", verdict: "none", proposer: "x" }))).toMatchObject({ filter: { itemType: "claim", verdict: "none", proposer: "x" } });
    expect(parseReviewRoute([], query({ verdict: "maybe" }))).toMatchObject({ filter: { verdict: "" } });
    expect(parseReviewRoute(["-", "changesets"], query({ state: "bogus" }))).toEqual({ kind: "changesets", state: "" });
  });
  it("lands on unknown for what no link makes", () => {
    for (const rest of [["-"], ["-", "nope"], ["cs", "more"], ["-", "contradictions", "x"], ["-", "changesets", "a", "b"]]) expect(parseReviewRoute(rest, none)).toEqual({ kind: "unknown" });
  });
  it("makes the paths it reads, encoding what it must", () => {
    const filter = { ...NO_FILTER, itemType: "entity:a b", verdict: "reject" };
    expect(reviewPath(vault, { kind: "queue", filter, item: "c/1" })).toBe("/vaults/v/review/c%2F1?type=entity%3Aa%20b&verdict=reject");
    expect(reviewPath(vault, { kind: "queue", filter: NO_FILTER, item: null })).toBe("/vaults/v/review");
    expect(reviewPath(vault, { kind: "changesets", state: "committed" })).toBe("/vaults/v/review/-/changesets?state=committed");
    expect(reviewPath(vault, { kind: "changeset", id: "c" })).toBe("/vaults/v/review/-/changesets/c");
    expect(reviewPath(vault, { kind: "auto_commit" })).toBe("/vaults/v/review/-/auto-commit");
    expect(reviewPath(vault, { kind: "contradictions" })).toBe("/vaults/v/review/-/contradictions");
  });
});
