import { describe, expect, it } from "vitest";
import type { components } from "./generated/schema";
import { claimLineageFromWire, flaggedPagesFromWire, pageWordsFromWire, renditionFromWire, transcriptFromWire } from "./translate-documents";

type Schemas = components["schemas"];

/** These fields are free-form in the document, which types them as having no known keys. */
const free = (value: unknown): never => value as never;

const rendition: Schemas["RenditionDto"] = { id: "r", source_id: "s", parser: "p", pipeline_version: 1, page_count: 1, segment_count: 1, created_at_ms: 5, pages: [] };

describe("a rendition's pages", () => {
  it("read each flag", () => {
    const wire: Schemas["RenditionDto"] = {
      ...rendition,
      pages: [free({ index: 2, route: "ocr", mean_confidence_permille: 900, flags: [{ flag: "hidden_text", words: 3 }, { flag: "low_confidence" }, { flag: "ocr_failed", reason: "boom" }, { flag: "invisible_unicode", chars: 2 }, { flag: "zero_size_text", words: 1 }] })],
    };
    const { pages, supersededBy } = renditionFromWire(wire);
    expect(supersededBy).toBeNull();
    expect(pages[0]).toEqual({
      index: 2, route: "ocr", meanConfidencePermille: 900,
      flags: [{ kind: "hidden_text", words: 3 }, { kind: "low_confidence" }, { kind: "ocr_failed", reason: "boom" }, { kind: "invisible_unicode", chars: 2 }, { kind: "zero_size_text", words: 1 }],
    });
  });

  it("survive what they don't expect, as these fields have no schema", () => {
    const { pages } = renditionFromWire({ ...rendition, pages: [free({ flags: [{ flag: "newly_invented" }, "nonsense"] }), free("nonsense")] });
    expect(pages[0]?.flags).toEqual([{ kind: "other", name: "newly_invented" }, { kind: "other", name: "unknown" }]);
    expect(pages[1]).toEqual({ index: 0, route: "unknown", meanConfidencePermille: null, flags: [] });
  });

  it("read the same on the flagged pages route", () => {
    const page = flaggedPagesFromWire({ items: [{ source_id: "s", rendition_id: "r", page: 4, outcome: free({ flags: [{ flag: "low_confidence" }] }) }], next_cursor: "c" });
    expect(page).toEqual({ items: [{ sourceId: "s", renditionId: "r", page: 4, flags: [{ kind: "low_confidence" }] }], nextCursor: "c" });
  });
});

describe("a word table", () => {
  const detail = (word_table: unknown): Schemas["RenditionDetail"] => ({ rendition, canonical_text: "", word_table: free(word_table) });

  it("gives words with their rects and quads", () => {
    const words = pageWordsFromWire(detail({ words: [{ text: "a", page: 1, rect: { x0: 1, y0: 2, x1: 3, y1: 4 }, quad: [[0, 0], [1, 0], [1, 1], [0, 1]] }, { text: "b", page: 1, rect: { x0: 1, y0: 2, x1: 3, y1: 4 } }] }));
    expect(words[0]).toEqual({ text: "a", page: 1, rect: { x0: 1, y0: 2, x1: 3, y1: 4 }, quad: [[0, 0], [1, 0], [1, 1], [0, 1]] });
    expect(words[1]?.quad).toBeNull();
  });

  it("is empty for plain text, which has none", () => {
    expect(pageWordsFromWire(detail(null))).toEqual([]);
  });

  it("is empty for a recording, whose words have times and no rects", () => {
    expect(pageWordsFromWire(detail({ words: [{ text: "a", start_ms: 0, end_ms: 1 }] }))).toEqual([]);
  });

  it("drops a word it can't place and a quad it can't read", () => {
    const words = pageWordsFromWire(detail({ words: [{ text: 1, page: 1 }, { text: "a", page: 0, rect: { x0: "x" } }, { text: "b", page: 0, rect: { x0: 1, y0: 2, x1: 3, y1: 4 }, quad: [[0, 0], ["a", 0], [1, 1], [0, 1]] }] }));
    expect(words).toHaveLength(1);
    expect(words[0]?.quad).toBeNull();
  });
});

describe("a claim's lineage", () => {
  it("reads the claim's row, which has no schema, and its evidence", () => {
    const lineage = claimLineageFromWire("a1", {
      assertion: free({ subject: "the pump", predicate: "opened in", value: 1912, epistemic: "Asserted", superseded_by: "a2" }),
      changeset_id: "cs",
      proposer: "pat",
      proposer_is_machine: false,
      evidence: [{ evidence_id: "e", quote: "<b>q</b>", stance: "supports", segment_id: "seg", rendition_id: "r", parser: "p", source_id: "s", source_name: "n", source_sha256: "h", trust_tier: "trusted", superseded: true, broken: false, rects: [{ page: 1, x0: 1, y0: 2, x1: 3, y1: 4 }], media_type: "application/pdf" }],
    });
    expect(lineage).toMatchObject({ claimId: "a1", subject: "the pump", value: "1912", epistemic: "Asserted", supersededBy: "a2", decidedBy: null, proposerIsMachine: false });
    expect(lineage.evidence[0]).toMatchObject({ quote: "<b>q</b>", superseded: true, broken: false, tStartMs: null, rects: [{ page: 1, x0: 1, y0: 2, x1: 3, y1: 4 }] });
  });

  it("gives a recording's time", () => {
    const lineage = claimLineageFromWire("a1", { assertion: free({}), changeset_id: "cs", evidence: [{ evidence_id: "e", quote: "q", stance: "supports", segment_id: "seg", rendition_id: "r", parser: "p", source_id: "s", source_name: "n", source_sha256: "h", trust_tier: "trusted", superseded: false, broken: false, rects: [], media_type: "audio/mpeg", t_start_ms: 1000, t_end_ms: 2000, speaker: "A" }] });
    expect(lineage.subject).toBeNull();
    expect(lineage.value).toBe("");
    expect(lineage.evidence[0]).toMatchObject({ tStartMs: 1000, tEndMs: 2000, speaker: "A" });
  });
});

describe("a transcript", () => {
  it("reads parts and words, with none when they weren't asked for", () => {
    const t = transcriptFromWire({ rendition_id: "r", parser: "p", duration_ms: 9, next_cursor: "30", parts: [{ start: 0, end: 3, text: "a b", t_start_ms: 1, t_end_ms: 2, words: [{ text: "a", start_ms: 1, end_ms: 2 }] }, { start: 4, end: 5, text: "c", t_start_ms: 3, t_end_ms: 4, speaker: "S" }] });
    expect(t.nextCursor).toBe("30");
    expect(t.parts[0]).toEqual({ start: 0, text: "a b", tStartMs: 1, tEndMs: 2, speaker: null, words: [{ text: "a", startMs: 1, endMs: 2 }] });
    expect(t.parts[1]?.speaker).toBe("S");
    expect(t.parts[1]?.words).toEqual([]);
  });
});
