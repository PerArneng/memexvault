import type { EpochMs } from "./index";

/** A box in displayed coordinates, after CropBox and /Rotate: origin top left, y down, integer 1/100 pt. */
export interface PageRect {
  readonly x0: number;
  readonly y0: number;
  readonly x1: number;
  readonly y1: number;
}

export interface AnchorRect extends PageRect {
  /** 0-based. */
  readonly page: number;
}

/** A point of an OCR line's quad, in the same units as `PageRect`. */
export type QuadPoint = readonly [number, number];
export type Quad = readonly [QuadPoint, QuadPoint, QuadPoint, QuadPoint];

/** The console's name for an original file. All of its text is untrusted. */
export interface DocumentSource {
  readonly id: string;
  readonly name: string;
  readonly sha256: string;
  readonly mediaType: string;
  readonly trustTier: string;
  readonly size: number;
  readonly logicalId: string;
  readonly versionNo: number;
  /** What an edit bases itself on, as distinct from `versionNo`. */
  readonly version: number;
  readonly createdBy: string;
  readonly recordedAt: EpochMs;
}

export type PageFlag =
  | { readonly kind: "low_confidence" }
  | { readonly kind: "hidden_text"; readonly words: number }
  | { readonly kind: "zero_size_text"; readonly words: number }
  | { readonly kind: "invisible_unicode"; readonly chars: number }
  | { readonly kind: "ocr_failed"; readonly reason: string }
  /** Something a newer server knows about and this console doesn't. */
  | { readonly kind: "other"; readonly name: string };

export interface PageOutcome {
  readonly index: number;
  readonly route: string;
  readonly meanConfidencePermille: number | null;
  readonly flags: readonly PageFlag[];
}

export interface Rendition {
  readonly id: string;
  readonly sourceId: string;
  readonly parser: string;
  readonly pipelineVersion: number;
  readonly pageCount: number;
  readonly segmentCount: number;
  readonly createdAt: EpochMs;
  readonly supersededBy: string | null;
  readonly pages: readonly PageOutcome[];
}

export interface PageFact {
  readonly index: number;
  /** Displayed size in 1/100 pt, after CropBox and /Rotate. */
  readonly width: number;
  readonly height: number;
  readonly rotation: number;
}

export interface PageList {
  readonly renditionId: string;
  readonly allowedDpi: readonly number[];
  readonly pages: readonly PageFact[];
}

export interface PageWord {
  readonly text: string;
  readonly page: number;
  readonly rect: PageRect;
  readonly quad: Quad | null;
}

export interface Segment {
  readonly id: string;
  readonly sourceId: string;
  readonly renditionId: string | null;
  readonly text: string;
  readonly heading: string | null;
  readonly language: string | null;
  readonly pageFrom: number | null;
  readonly pageTo: number | null;
  readonly speaker: string | null;
  readonly tStartMs: number | null;
  readonly tEndMs: number | null;
}

export interface FlaggedPage {
  readonly sourceId: string;
  readonly renditionId: string;
  readonly page: number;
  readonly flags: readonly PageFlag[];
}

/** A claim that cites a source or segment. */
export interface CitingClaim {
  readonly assertionId: string;
  readonly evidenceId: string;
  readonly subject: string;
  readonly predicate: string;
  readonly value: string;
}

export interface EvidenceStep {
  readonly evidenceId: string;
  readonly quote: string;
  readonly stance: string;
  readonly segmentId: string;
  readonly renditionId: string;
  readonly parser: string;
  readonly sourceId: string;
  readonly sourceName: string;
  readonly sourceSha256: string;
  readonly trustTier: string;
  readonly mediaType: string;
  /** A newer rendition exists. This still resolves. */
  readonly superseded: boolean;
  /** The quote no longer resolves. The server reports it and guesses nothing. */
  readonly broken: boolean;
  readonly rects: readonly AnchorRect[];
  readonly tStartMs: number | null;
  readonly tEndMs: number | null;
  readonly speaker: string | null;
}

export interface ClaimLineage {
  readonly claimId: string;
  readonly subject: string | null;
  readonly predicate: string | null;
  readonly value: string;
  readonly epistemic: string | null;
  readonly supersededBy: string | null;
  readonly changesetId: string;
  readonly proposer: string | null;
  readonly proposerIsMachine: boolean | null;
  readonly decidedBy: string | null;
  readonly rationale: string | null;
  readonly evidence: readonly EvidenceStep[];
}

/** What's known of how a change was made: the model behind an extraction, and the prompt it ran on. */
export interface ChangeOrigin {
  readonly extractor: string | null;
  readonly promptVersion: string | null;
  readonly state: string;
}

export interface Derivation {
  readonly id: string;
  readonly kind: string;
  readonly model: string;
  readonly promptHash: string;
  readonly outcome: string;
  readonly segmentIds: readonly string[];
  readonly createdAt: EpochMs;
}

export interface MediaFacts {
  readonly sourceId: string;
  readonly mediaType: string;
  readonly size: number;
  readonly renditionId: string | null;
  readonly durationMs: number | null;
  readonly gaps: number;
  readonly skippedSpans: number;
  readonly coarseSpans: number;
}

export interface TranscriptWord {
  readonly text: string;
  readonly startMs: number;
  readonly endMs: number;
}

export interface TranscriptPart {
  readonly start: number;
  readonly text: string;
  readonly tStartMs: number;
  readonly tEndMs: number;
  readonly speaker: string | null;
  readonly words: readonly TranscriptWord[];
}

export interface Transcript {
  readonly renditionId: string;
  readonly parser: string;
  readonly durationMs: number;
  readonly parts: readonly TranscriptPart[];
  readonly nextCursor: string | null;
}
