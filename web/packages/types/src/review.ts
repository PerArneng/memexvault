import type { EpochMs } from "./index";
import type { AnchorRect, PageFlag } from "./documents";

/** A proposal and what became of it. Its rationale, ops and reasons are untrusted, as a proposer or a document wrote them. */
export interface ChangeSet {
  readonly id: string;
  /** `pending`, `committed` or `rejected`. */
  readonly state: string;
  readonly proposer: string;
  readonly proposerIsMachine: boolean;
  /** The person whose chat asked for this, when the chat agent proposed it. */
  readonly requestedBy: string | null;
  readonly rationale: string;
  readonly ops: readonly unknown[];
  /** What was proposed, when a person changed it before approving. */
  readonly originalOps: readonly unknown[] | null;
  readonly validators: readonly unknown[];
  /** `auto_commit`, `review` or `reject`. */
  readonly risk: string;
  readonly proposedAt: EpochMs;
  readonly decidedAt: EpochMs | null;
  readonly decidedBy: string | null;
  readonly decisionReason: string | null;
  readonly extractor: string | null;
  readonly promptVersion: string | null;
  readonly itemTypes: readonly string[];
  readonly sourceIds: readonly string[];
  readonly reverts: string | null;
  readonly revertedBy: string | null;
}

/** The reviewer agent's opinion. It's advice, and it decides nothing. */
export interface ReviewVerdict {
  /** `approve`, `reject` or `abstain`. */
  readonly verdict: string;
  /** The model's own words about untrusted text. */
  readonly reason: string;
  readonly model: string;
  readonly createdAt: EpochMs;
}

export interface ReviewItem {
  readonly changeset: ChangeSet;
  readonly verdict: ReviewVerdict | null;
}

export interface ReviewEvidence {
  readonly claimId: string;
  readonly segmentId: string;
  readonly quote: string;
  readonly sourceId: string | null;
  readonly sourceName: string | null;
  readonly trustTier: string | null;
  readonly renditionId: string | null;
  readonly pageFrom: number | null;
  readonly pageTo: number | null;
  /** Its document was processed again, so this can't be approved as it stands. */
  readonly superseded: boolean;
  readonly pageFlags: readonly PageFlag[];
  /** Where the quote is on its pages. Empty when it has no place on a page. */
  readonly rects: readonly AnchorRect[];
}

export interface AutoCommitChange {
  readonly at: EpochMs;
  readonly by: string;
  /** `earned`, `fell_below` or `disabled`. */
  readonly kind: string;
  readonly reason: string;
  readonly correct: number;
  readonly incorrect: number;
}

/** One combination of extractor, prompt version and item type, and where it stands. */
export interface AutoCommitEntry {
  readonly id: string;
  readonly extractor: string;
  readonly promptVersion: string;
  readonly itemType: string;
  readonly correct: number;
  readonly incorrect: number;
  readonly totalCorrect: number;
  readonly totalIncorrect: number;
  readonly precision: number | null;
  readonly wilsonBound: number;
  readonly on: boolean;
  /** Oldest first. */
  readonly history: readonly AutoCommitChange[];
}

export interface AutoCommitTable {
  readonly threshold: number;
  readonly entries: readonly AutoCommitEntry[];
}

export interface ReviewDetail extends ReviewItem {
  readonly evidence: readonly ReviewEvidence[];
  readonly combinations: readonly AutoCommitEntry[];
  /** Only a person can approve this. */
  readonly needsAPerson: boolean;
  /** Why auto-commit wouldn't take this even if earned, in the server's words. */
  readonly autoCommitBlock: string | null;
}

export interface QueueCountGroup {
  readonly extractor: string;
  readonly promptVersion: string;
  readonly itemType: string;
  readonly verdict: string | null;
  readonly count: number;
}

export interface QueueCounts {
  readonly pending: number;
  readonly groups: readonly QueueCountGroup[];
}

export interface InversePreview {
  readonly reversible: boolean;
  readonly blockedBecause: string | null;
  readonly mutations: readonly unknown[];
}

export interface Contradiction {
  readonly claim: string;
  readonly other: string;
}

export type ReviewDecision =
  | { readonly kind: "approve"; readonly id: string; readonly reason: string | null }
  | { readonly kind: "reject"; readonly id: string; readonly reason: string }
  | { readonly kind: "edit_approve"; readonly id: string; readonly ops: readonly unknown[]; readonly reason: string | null };

/** How one decision in a batch went. */
export interface BatchOutcome {
  readonly id: string;
  readonly changeset: ChangeSet | null;
  readonly errorCode: string | null;
  readonly errorMessage: string | null;
}

export interface ReviewFilter {
  readonly extractor: string;
  readonly promptVersion: string;
  readonly itemType: string;
  readonly proposer: string;
  readonly verdict: string;
  readonly sourceId: string;
}
