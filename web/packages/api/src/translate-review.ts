import { epochMs } from "@memex/types";
import type { AutoCommitChange, AutoCommitEntry, BatchOutcome, ChangeSet, Contradiction, InversePreview, Page, QueueCounts, ReviewDetail, ReviewEvidence, ReviewFilter, ReviewItem, ReviewVerdict } from "@memex/types";
import type { components } from "./generated/schema";
import { flagFromWire } from "./translate-documents";

type Schemas = components["schemas"];

export const changesetFromWire = (c: Schemas["ChangeSetDto"]): ChangeSet => ({
  id: c.id,
  state: c.state,
  proposer: c.proposer,
  proposerIsMachine: c.proposer_is_machine,
  requestedBy: c.requested_by ?? null,
  rationale: c.rationale,
  ops: c.ops,
  originalOps: c.original_ops ?? null,
  validators: c.validators,
  risk: c.risk,
  proposedAt: epochMs(c.proposed_at_ms),
  decidedAt: c.decided_at_ms === undefined || c.decided_at_ms === null ? null : epochMs(c.decided_at_ms),
  decidedBy: c.decided_by ?? null,
  decisionReason: c.decision_reason ?? null,
  extractor: c.extractor ?? null,
  promptVersion: c.prompt_version ?? null,
  itemTypes: c.item_types ?? [],
  sourceIds: c.source_ids ?? [],
  reverts: c.reverts ?? null,
  revertedBy: c.reverted_by ?? null,
});

const verdictFromWire = (v: Schemas["ReviewVerdictDto"]): ReviewVerdict => ({ verdict: v.verdict, reason: v.reason, model: v.model, createdAt: epochMs(v.created_at_ms) });

const itemFromWire = (i: Schemas["ReviewItemDto"]): ReviewItem => ({ changeset: changesetFromWire(i.changeset), verdict: i.verdict === undefined || i.verdict === null ? null : verdictFromWire(i.verdict) });

export const reviewPageFromWire = (p: Schemas["ReviewItemPage"]): Page<ReviewItem> => ({ items: p.items.map(itemFromWire), nextCursor: p.next_cursor ?? null });

export const changesetPageFromWire = (p: Schemas["ChangeSetPage"]): Page<ChangeSet> => ({ items: p.items.map(changesetFromWire), nextCursor: p.next_cursor ?? null });

const evidenceFromWire = (e: Schemas["ReviewEvidenceDto"]): ReviewEvidence => ({
  claimId: e.claim_id,
  segmentId: e.segment_id,
  quote: e.quote,
  sourceId: e.source_id ?? null,
  sourceName: e.source_name ?? null,
  trustTier: e.trust_tier ?? null,
  renditionId: e.rendition_id ?? null,
  pageFrom: e.page_from ?? null,
  pageTo: e.page_to ?? null,
  superseded: e.superseded,
  pageFlags: e.page_flags.map(flagFromWire),
  rects: (e.rects ?? []).map((r) => ({ page: r.page, x0: r.x0, y0: r.y0, x1: r.x1, y1: r.y1 })),
});

const changeFromWire = (c: Schemas["AutoCommitChangeDto"]): AutoCommitChange => ({ at: epochMs(c.at_ms), by: c.by, kind: c.kind, reason: c.reason, correct: c.correct, incorrect: c.incorrect });

export const autoCommitFromWire = (a: Schemas["AutoCommitDto"]): AutoCommitEntry => ({
  id: a.id,
  extractor: a.extractor,
  promptVersion: a.prompt_version,
  itemType: a.item_type,
  correct: a.correct,
  incorrect: a.incorrect,
  totalCorrect: a.total_correct,
  totalIncorrect: a.total_incorrect,
  precision: a.precision ?? null,
  wilsonBound: a.wilson_bound,
  on: a.on,
  history: a.history.map(changeFromWire),
});

export const reviewDetailFromWire = (d: Schemas["ReviewDetailDto"]): ReviewDetail => ({
  changeset: changesetFromWire(d.changeset),
  verdict: d.verdict === undefined || d.verdict === null ? null : verdictFromWire(d.verdict),
  evidence: d.evidence.map(evidenceFromWire),
  combinations: d.combinations.map(autoCommitFromWire),
  needsAPerson: d.needs_a_person,
  autoCommitBlock: d.auto_commit_block ?? null,
});

export const queueCountsFromWire = (c: Schemas["QueueCountsDto"]): QueueCounts => ({
  pending: c.pending,
  groups: c.groups.map((g) => ({ extractor: g.extractor, promptVersion: g.prompt_version, itemType: g.item_type, verdict: g.verdict ?? null, count: g.count })),
});

export const inverseFromWire = (i: Schemas["InversePreview"]): InversePreview => ({ reversible: i.reversible, blockedBecause: i.blocked_because ?? null, mutations: i.mutations });

export const contradictionPageFromWire = (p: Schemas["ContradictionPage"]): Page<Contradiction> => ({ items: p.items.map((c) => ({ claim: c.claim, other: c.other })), nextCursor: p.next_cursor ?? null });

export const batchFromWire = (b: Schemas["BatchResultsDto"]): readonly BatchOutcome[] =>
  b.results.map((r) => ({ id: r.id, changeset: r.changeset === undefined || r.changeset === null ? null : changesetFromWire(r.changeset), errorCode: r.error_code ?? null, errorMessage: r.error_message ?? null }));

/** Only what's set goes on the wire, as the server treats an empty value as a value. */
export const reviewFilterToWire = (filter: ReviewFilter): Readonly<Record<string, string>> => {
  const wire: Record<string, string> = {};
  if (filter.extractor !== "") wire.extractor = filter.extractor;
  if (filter.promptVersion !== "") wire.prompt_version = filter.promptVersion;
  if (filter.itemType !== "") wire.item_type = filter.itemType;
  if (filter.proposer !== "") wire.proposer = filter.proposer;
  if (filter.verdict !== "") wire.verdict = filter.verdict;
  if (filter.sourceId !== "") wire.source_id = filter.sourceId;
  return wire;
};
