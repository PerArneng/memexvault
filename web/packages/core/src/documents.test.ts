import { epochMs, vaultId } from "@memex/types";
import type { DocumentSource, TranscriptPart } from "@memex/types";
import { describe, expect, it } from "vitest";
import { activePart, canChangeSource, evidenceState, filterSources, flagText, flaggedSources, formatClock, formatSize, mediaKind, pageAngle, pageWordsByPage, partsInRange, playability, trustEffect, wordAt } from "./documents";
import { identityWith } from "./testing";

const source = (over: Partial<DocumentSource>): DocumentSource => ({
  id: "s", name: "a.pdf", sha256: "x", mediaType: "application/pdf", trustTier: "unverified", size: 1, logicalId: "s", versionNo: 1, version: 1, createdBy: "p", recordedAt: epochMs(0), ...over,
});
const NONE = { text: "", tier: "", kind: "", flagged: false };

describe("the source list's filters", () => {
  const sources = [source({ id: "1", name: "Report.PDF" }), source({ id: "2", name: "talk.mp3", mediaType: "audio/mpeg", trustTier: "trusted" }), source({ id: "3", name: "notes.txt", mediaType: "text/plain" })];

  it("show everything with none set", () => {
    expect(filterSources(sources, NONE, new Set())).toHaveLength(3);
  });

  it("match a name whatever its case", () => {
    expect(filterSources(sources, { ...NONE, text: " report " }, new Set()).map((s) => s.id)).toEqual(["1"]);
  });

  it("filter by tier, kind and flags, and together", () => {
    expect(filterSources(sources, { ...NONE, tier: "trusted" }, new Set()).map((s) => s.id)).toEqual(["2"]);
    expect(filterSources(sources, { ...NONE, kind: "text" }, new Set()).map((s) => s.id)).toEqual(["3"]);
    expect(filterSources(sources, { ...NONE, flagged: true }, flaggedSources([{ sourceId: "1", renditionId: "r", page: 0, flags: [] }])).map((s) => s.id)).toEqual(["1"]);
    expect(filterSources(sources, { ...NONE, kind: "audio", tier: "untrusted" }, new Set())).toEqual([]);
  });

  it("tell media apart", () => {
    expect(["application/pdf", "text/markdown", "audio/ogg", "video/mp4", "application/zip"].map(mediaKind)).toEqual(["pdf", "text", "audio", "video", "other"]);
  });
});

describe("who may change a source", () => {
  const v = vaultId("v");
  it("needs propose scope, as the server does", () => {
    expect(canChangeSource(identityWith([{ vault: v, scope: "read" }]), v)).toBe(false);
    expect(canChangeSource(identityWith([{ vault: v, scope: "propose" }]), v)).toBe(true);
    expect(canChangeSource(identityWith([{ vault: v, scope: "admin" }]), v)).toBe(true);
    expect(canChangeSource(identityWith([]), v)).toBe(false);
  });
});

describe("what a trust change does", () => {
  it("is nothing to the same tier", () => {
    expect(trustEffect("trusted", "trusted")).toBe("same");
  });

  it("from untrusted always waits for a person", () => {
    expect(trustEffect("untrusted", "unverified")).toBe("review_always");
    expect(trustEffect("untrusted", "trusted")).toBe("review_always");
  });

  it("to trusted or above commits for a person, and waits for an agent", () => {
    expect(trustEffect("unverified", "trusted")).toBe("person_commits_agent_waits");
    expect(trustEffect("unverified", "verified")).toBe("person_commits_agent_waits");
  });

  it("anywhere else lowers it, or stays above the bar", () => {
    expect(trustEffect("trusted", "unverified")).toBe("lowers");
    expect(trustEffect("verified", "untrusted")).toBe("lowers");
    expect(trustEffect("trusted", "verified")).toBe("lowers");
  });
});

describe("an evidence row's state", () => {
  it("says broken first, as an old rendition can be both", () => {
    expect(evidenceState({ broken: true, superseded: true })).toBe("broken");
    expect(evidenceState({ broken: false, superseded: true })).toBe("superseded");
    expect(evidenceState({ broken: false, superseded: false })).toBe("current");
  });
});

describe("flags", () => {
  it("each say something, including one this console doesn't know", () => {
    expect(flagText({ kind: "low_confidence" })).toContain("low confidence");
    expect(flagText({ kind: "hidden_text", words: 3 })).toContain("3 words");
    expect(flagText({ kind: "zero_size_text", words: 2 })).toContain("zero-size");
    expect(flagText({ kind: "invisible_unicode", chars: 4 })).toContain("4 invisible");
    expect(flagText({ kind: "ocr_failed", reason: "boom" })).toContain("boom");
    expect(flagText({ kind: "other", name: "new_flag" })).toBe("new_flag");
  });
});

describe("a word's direction on the page", () => {
  it("follows the turn for a text layer, and not for OCR", () => {
    const rendition = { pages: [{ index: 0, route: "text_layer", meanConfidencePermille: null, flags: [] }, { index: 1, route: "ocr", meanConfidencePermille: 900, flags: [] }] };
    expect(pageAngle(rendition, { index: 0, rotation: 90 })).toBe(90);
    expect(pageAngle(rendition, { index: 1, rotation: 90 })).toBe(0);
    expect(pageAngle(null, { index: 0, rotation: 270 })).toBe(270);
  });

  it("groups words by page, in order", () => {
    const word = (text: string, page: number) => ({ text, page, rect: { x0: 0, y0: 0, x1: 1, y1: 1 }, quad: null });
    const map = pageWordsByPage([word("a", 0), word("b", 1), word("c", 0)]);
    expect(map.get(0)?.map((w) => w.text)).toEqual(["a", "c"]);
    expect(map.get(1)?.map((w) => w.text)).toEqual(["b"]);
    expect(map.get(2)).toBeUndefined();
  });
});

describe("a transcript's sync", () => {
  const part = (tStartMs: number, tEndMs: number): TranscriptPart => ({ start: 0, text: "x", tStartMs, tEndMs, speaker: null, words: [] });
  const parts = [part(1000, 2000), part(3000, 4000), part(5000, 6000)];

  it("finds the part playing", () => {
    expect(activePart(parts, 1500)).toBe(0);
    expect(activePart(parts, 3000)).toBe(1);
    expect(activePart(parts, 99999)).toBe(2);
  });

  it("stays on the last part through a pause, and has none before the first", () => {
    expect(activePart(parts, 2500)).toBe(0);
    expect(activePart(parts, 500)).toBeNull();
    expect(activePart([], 500)).toBeNull();
  });

  it("finds the word playing, and none between words", () => {
    const words = [{ text: "a", startMs: 0, endMs: 100 }, { text: "b", startMs: 200, endMs: 300 }];
    expect(wordAt(words, 50)).toBe(0);
    expect(wordAt(words, 150)).toBeNull();
    expect(wordAt(words, 299)).toBe(1);
    expect(wordAt(words, 300)).toBeNull();
  });

  it("marks the parts a cited range touches", () => {
    expect(partsInRange(parts, 1500, 3500)).toEqual([0, 1]);
    expect(partsInRange(parts, 2100, 2900)).toEqual([]);
    expect(partsInRange(parts, 5500, null)).toEqual([2]);
  });
});

describe("formats", () => {
  it("write a clock, with hours only when there are some", () => {
    expect(formatClock(0)).toBe("0:00");
    expect(formatClock(61_500)).toBe("1:01");
    expect(formatClock(3_725_000)).toBe("1:02:05");
    expect(formatClock(-5)).toBe("0:00");
  });

  it("write a size", () => {
    expect(formatSize(12)).toBe("12 B");
    expect(formatSize(2048)).toBe("2.0 KB");
    expect(formatSize(5 * 1024 * 1024)).toBe("5.0 MB");
  });

  it("read what a browser says about playing", () => {
    expect(playability("")).toBe("no");
    expect(playability("maybe")).toBe("try");
    expect(playability("probably")).toBe("yes");
  });
});
