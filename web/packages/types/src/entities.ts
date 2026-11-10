import type { EpochMs } from "./index";

/** One version of an entity type. The schema comes from the vault, so it's untrusted and may be anything. */
export interface EntityType {
  readonly name: string;
  readonly version: number;
  readonly schema: unknown;
  /** `seed`, `new`, `additive` or `breaking`. */
  readonly change: string;
  readonly reasons: readonly string[];
  readonly changesetId: string | null;
  readonly createdAt: EpochMs;
}

/** What submitting a schema would do for this caller. Nothing is written. */
export interface EntityTypePreview {
  /** Why a submission would be refused. */
  readonly problem: string | null;
  /** `new`, `additive` or `breaking`. */
  readonly change: string;
  readonly reasons: readonly string[];
  readonly currentVersion: number | null;
  readonly nextVersion: number;
  /** `auto_commit`, `review` or `reject`. */
  readonly decision: string;
}

/** An entity as a list shows it. Attributes are vault content, so they're untrusted. */
export interface EntitySummary {
  readonly id: string;
  readonly typeName: string;
  readonly typeVersion: number;
  readonly version: number;
  readonly aliases: readonly string[];
  readonly attributes: Readonly<Record<string, unknown>>;
  readonly conflictedFields: readonly string[];
  readonly mergedFrom: readonly string[];
  readonly updatedAt: EpochMs;
}

export interface FieldEvidence {
  readonly evidenceId: string;
  readonly quote: string;
  readonly stance: string;
  readonly segmentId: string;
  readonly sourceId: string | null;
  readonly sourceName: string | null;
  readonly trustTier: string | null;
  readonly renditionId: string | null;
  readonly pageFrom: number | null;
  readonly pageTo: number | null;
  readonly anchorBroken: boolean;
}

/** A claim on a field. Null facts are a claim whose row can't be read any more. */
export interface FieldClaim {
  readonly assertionId: string;
  readonly value: unknown;
  readonly epistemic: string | null;
  readonly machine: boolean | null;
  readonly creator: string | null;
  readonly changesetId: string | null;
  readonly evidence: readonly FieldEvidence[];
}

export interface EntityFieldDetail {
  readonly field: string;
  readonly value: unknown;
  /** Why the current value is the one chosen. */
  readonly why: string;
  readonly current: FieldClaim;
  readonly conflicts: readonly FieldClaim[];
  readonly agreeing: readonly FieldClaim[];
}

export interface EntityDetail {
  readonly id: string;
  readonly typeName: string;
  readonly typeVersion: number;
  readonly version: number;
  readonly aliases: readonly string[];
  readonly mergedFrom: readonly string[];
  /** The id asked for, when it has been merged into this one. */
  readonly redirectedFrom: string | null;
  readonly changesetId: string;
  readonly updatedAt: EpochMs;
  readonly fields: readonly EntityFieldDetail[];
}

export interface EntityRedirect {
  readonly id: string;
  readonly currentId: string;
  readonly chain: readonly string[];
}

export interface EntityCandidate {
  readonly entity: EntitySummary;
  readonly matchedOn: string | null;
  readonly exact: boolean;
  readonly score: number;
}

export interface EntityResolution {
  readonly candidates: readonly EntityCandidate[];
  /** `lexical`, if only exact matches could be looked for. */
  readonly degraded: readonly string[];
}

/** A suggestion, never a decision: these entities share an identity value or alias. */
export interface SameAsSuggestion {
  readonly field: string;
  readonly value: string;
  readonly entityIds: readonly string[];
}

/** One link at an object, from the neighborhood API. */
export interface Neighbor {
  readonly edgeId: string;
  readonly kind: string;
  readonly direction: string;
  readonly from: string;
  readonly to: string;
  /** What's at the other end. Null is a link to a slug no page holds. */
  readonly neighbor: string | null;
  readonly title: string | null;
}
