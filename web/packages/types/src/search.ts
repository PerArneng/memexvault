import type { AnchorRect, PageWord } from "./documents";

/** Where an item's text comes from. A segment gives offsets and pages, and a claim its stored rectangles. */
export interface SearchCitation {
  /** `located`, `broken` or `none`, or what a newer server says. */
  readonly anchor: string;
  readonly sourceId: string | null;
  readonly sourceName: string | null;
  readonly sourceSha256: string | null;
  readonly trustTier: string | null;
  readonly segmentId: string | null;
  readonly evidenceId: string | null;
  readonly renditionId: string | null;
  readonly pageFrom: number | null;
  readonly pageTo: number | null;
  readonly rects: readonly AnchorRect[];
  readonly quote: string | null;
  readonly tStartMs: number | null;
  readonly tEndMs: number | null;
  readonly speaker: string | null;
}

/** A claim that disagrees with an item. */
export interface SearchConflict {
  readonly claimId: string;
  readonly text: string;
  readonly epistemic: string | null;
  readonly sourceIds: readonly string[];
}

export interface EvidenceItem {
  readonly id: string;
  /** `claim`, `segment`, `entity`, `section`, `page` or `block`, or what a newer server says. */
  readonly kind: string;
  readonly text: string;
  readonly heading: string | null;
  readonly language: string | null;
  readonly epistemic: string | null;
  readonly trustTiers: readonly string[];
  readonly validFromMs: number | null;
  readonly validToMs: number | null;
  readonly score: number | null;
  /** The wiki page a page or block item belongs to. */
  readonly pageId: string | null;
  readonly citations: readonly SearchCitation[];
  readonly conflicts: readonly SearchConflict[];
}

export interface FacetCount {
  readonly value: string;
  readonly count: number;
}

export interface SearchFacets {
  readonly kind: readonly FacetCount[];
  readonly type: readonly FacetCount[];
  readonly sourceId: readonly FacetCount[];
  readonly trustTier: readonly FacetCount[];
  readonly epistemic: readonly FacetCount[];
}

export interface SignalScores {
  readonly id: string;
  readonly fused: number;
  readonly lexicalRank: number | null;
  readonly lexicalScore: number | null;
  readonly vectorRank: number | null;
  readonly vectorScore: number | null;
  readonly rerankScore: number | null;
}

export interface SearchDebug {
  readonly lexicalWeight: number;
  readonly vectorWeight: number;
  readonly lexicalTerms: number;
  readonly droppedTerms: number;
  readonly candidatesBeforeFence: number;
  readonly candidatesAfterFence: number;
  readonly expanded: number;
  readonly termCoverage: number | null;
  readonly bestVectorScore: number | null;
  readonly signals: readonly SignalScores[];
  readonly timings: readonly { readonly stage: string; readonly ms: number }[];
}

/** The answer to a search: claims first, then segments, then context sections. */
export interface EvidencePackage {
  /** `exact`, `identifier`, `natural`, `filtered` or `as_of`. */
  readonly path: string;
  readonly items: readonly EvidenceItem[];
  /** Nothing here looks like an answer. */
  readonly lowRelevance: boolean;
  readonly notices: readonly string[];
  readonly budget: { readonly limit: number; readonly used: number; readonly truncated: boolean };
  /** Signals that couldn't answer, so the result rests on the others. */
  readonly degraded: readonly string[];
  readonly facets: SearchFacets | null;
  readonly debug: SearchDebug | null;
  readonly nextCursor: string | null;
  readonly asOf: number | null;
}

/** A rendition's text and the words it was built from, for placing a stretch of the text on its pages. */
export interface RenditionReading {
  readonly canonicalText: string;
  readonly words: readonly PageWord[];
}
