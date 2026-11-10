import { describe, expect, it } from "vitest";
import type { components } from "./generated/schema";
import { evidencePackageFromWire, renditionReadingFromWire } from "./translate-search";

type Package = components["schemas"]["EvidencePackageDto"];

const wire: Package = {
  path: "natural",
  low_relevance: false,
  notices: ["n"],
  degraded: ["vector"],
  budget: { limit: 100, used: 10, truncated: true, smallest_item_needs: 5 },
  next_cursor: "r20",
  as_of: 9,
  facets: { kind: [{ value: "claim", count: 1 }], type: [], source_id: [{ value: "s1", count: 2 }], trust_tier: [], epistemic: [] },
  debug: {
    lexical_weight: 0.5, vector_weight: 0.5, lexical_terms: 2, dropped_terms: 1, candidates_before_fence: 4, candidates_after_fence: 3, expanded: 0, term_coverage: 0.5, best_vector_score: 0.9,
    signals: [{ id: "c1", fused: 0.1, lexical_rank: 1, lexical_score: 2, vector_rank: null, vector_score: null, rerank_score: null }],
    timings: [{ stage: "lexical", ms: 1.5 }],
  },
  items: [{
    id: "c1", kind: "claim", text: "räksmörgås Straße niño", heading: null, language: "sv", epistemic: "asserted", trust_tiers: ["trusted"], estimated_tokens: 5, valid_from_ms: 1, valid_to_ms: null, score: 0.5, page_id: null,
    citations: [{ anchor: "located", source_id: "s1", source_name: "a.pdf", source_sha256: "x", trust_tier: "trusted", segment_id: "seg1", evidence_id: "e1", rendition_id: "r1", start: 0, end: 5, page_from: 1, page_to: 2, rects: [{ page: 1, x0: 1, y0: 2, x1: 3, y1: 4 }], quote: "Straße", t_start_ms: null, t_end_ms: null, speaker: null }],
    conflicts: [{ claim_id: "c2", text: "other", epistemic: null, source_ids: ["s2"] }],
  }],
};

describe("evidencePackageFromWire", () => {
  it("translates everything, keeping text as it came", () => {
    const p = evidencePackageFromWire(wire);
    expect(p).toMatchObject({ path: "natural", lowRelevance: false, notices: ["n"], degraded: ["vector"], nextCursor: "r20", asOf: 9, budget: { limit: 100, used: 10, truncated: true } });
    expect(p.items[0]).toMatchObject({ id: "c1", text: "räksmörgås Straße niño", language: "sv", trustTiers: ["trusted"], validFromMs: 1, validToMs: null });
    expect(p.items[0]?.citations[0]).toMatchObject({ anchor: "located", sourceId: "s1", evidenceId: "e1", pageFrom: 1, pageTo: 2, quote: "Straße", rects: [{ page: 1, x0: 1, y0: 2, x1: 3, y1: 4 }] });
    expect(p.items[0]?.conflicts).toEqual([{ claimId: "c2", text: "other", epistemic: null, sourceIds: ["s2"] }]);
    expect(p.facets?.sourceId).toEqual([{ value: "s1", count: 2 }]);
    expect(p.debug).toMatchObject({ lexicalWeight: 0.5, termCoverage: 0.5, signals: [{ id: "c1", lexicalRank: 1, vectorRank: null }], timings: [{ stage: "lexical", ms: 1.5 }] });
  });

  it("reads what a server may leave out as nothing", () => {
    const p = evidencePackageFromWire({ path: wire.path, low_relevance: false, notices: [], degraded: [], budget: wire.budget, items: [{ id: "s", kind: "segment", text: "t", trust_tiers: [], estimated_tokens: 1, citations: [{ anchor: "none", rects: [] }], conflicts: [] }] });
    expect(p).toMatchObject({ facets: null, debug: null, nextCursor: null, asOf: null });
    expect(p.items[0]).toMatchObject({ heading: null, language: null, epistemic: null, score: null, pageId: null });
    expect(p.items[0]?.citations[0]).toMatchObject({ anchor: "none", sourceId: null, sourceName: null, quote: null, pageFrom: null, tStartMs: null });
  });
});

describe("renditionReadingFromWire", () => {
  it("takes the text and the words", () => {
    const reading = renditionReadingFromWire({ canonical_text: "a b", word_table: { words: [{ text: "a", page: 0, rect: { x0: 0, y0: 0, x1: 1, y1: 1 } }] } } as never);
    expect(reading.canonicalText).toBe("a b");
    expect(reading.words).toHaveLength(1);
  });

  it("has no words for plain text", () => {
    expect(renditionReadingFromWire({ canonical_text: "plain" } as never).words).toEqual([]);
  });
});
