import type { EvidenceItem, EvidencePackage, RenditionReading, SearchCitation, SearchConflict, SearchDebug, SearchFacets } from "@memex/types";
import type { components } from "./generated/schema";
import { pageWordsFromWire } from "./translate-documents";

type Schemas = components["schemas"];

const citationFromWire = (c: Schemas["CitationDto"]): SearchCitation => ({
  anchor: c.anchor,
  sourceId: c.source_id ?? null,
  sourceName: c.source_name ?? null,
  sourceSha256: c.source_sha256 ?? null,
  trustTier: c.trust_tier ?? null,
  segmentId: c.segment_id ?? null,
  evidenceId: c.evidence_id ?? null,
  renditionId: c.rendition_id ?? null,
  pageFrom: c.page_from ?? null,
  pageTo: c.page_to ?? null,
  rects: c.rects.map((r) => ({ page: r.page, x0: r.x0, y0: r.y0, x1: r.x1, y1: r.y1 })),
  quote: c.quote ?? null,
  tStartMs: c.t_start_ms ?? null,
  tEndMs: c.t_end_ms ?? null,
  speaker: c.speaker ?? null,
});

const conflictFromWire = (c: Schemas["ConflictDto"]): SearchConflict => ({ claimId: c.claim_id, text: c.text, epistemic: c.epistemic ?? null, sourceIds: c.source_ids });

const itemFromWire = (i: Schemas["EvidenceItemDto"]): EvidenceItem => ({
  id: i.id,
  kind: i.kind,
  text: i.text,
  heading: i.heading ?? null,
  language: i.language ?? null,
  epistemic: i.epistemic ?? null,
  trustTiers: i.trust_tiers,
  validFromMs: i.valid_from_ms ?? null,
  validToMs: i.valid_to_ms ?? null,
  score: i.score ?? null,
  pageId: i.page_id ?? null,
  citations: i.citations.map(citationFromWire),
  conflicts: i.conflicts.map(conflictFromWire),
});

const facetsFromWire = (f: Schemas["FacetsDto"]): SearchFacets => ({ kind: f.kind, type: f.type, sourceId: f.source_id, trustTier: f.trust_tier, epistemic: f.epistemic });

const debugFromWire = (d: Schemas["SearchDebugDto"]): SearchDebug => ({
  lexicalWeight: d.lexical_weight,
  vectorWeight: d.vector_weight,
  lexicalTerms: d.lexical_terms,
  droppedTerms: d.dropped_terms,
  candidatesBeforeFence: d.candidates_before_fence,
  candidatesAfterFence: d.candidates_after_fence,
  expanded: d.expanded,
  termCoverage: d.term_coverage ?? null,
  bestVectorScore: d.best_vector_score ?? null,
  signals: d.signals.map((s) => ({
    id: s.id,
    fused: s.fused,
    lexicalRank: s.lexical_rank ?? null,
    lexicalScore: s.lexical_score ?? null,
    vectorRank: s.vector_rank ?? null,
    vectorScore: s.vector_score ?? null,
    rerankScore: s.rerank_score ?? null,
  })),
  timings: d.timings,
});

export const evidencePackageFromWire = (p: Schemas["EvidencePackageDto"]): EvidencePackage => ({
  path: p.path,
  items: p.items.map(itemFromWire),
  lowRelevance: p.low_relevance,
  notices: p.notices,
  budget: { limit: p.budget.limit, used: p.budget.used, truncated: p.budget.truncated },
  degraded: p.degraded,
  facets: p.facets === null || p.facets === undefined ? null : facetsFromWire(p.facets),
  debug: p.debug === null || p.debug === undefined ? null : debugFromWire(p.debug),
  nextCursor: p.next_cursor ?? null,
  asOf: p.as_of ?? null,
});

export const renditionReadingFromWire = (detail: Schemas["RenditionDetail"]): RenditionReading => ({ canonicalText: detail.canonical_text, words: pageWordsFromWire(detail) });
