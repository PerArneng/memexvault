import { epochMs } from "@memex/types";
import type { AutoCommitEntry, AutoCommitTable, ChangeSet, Contradiction, InversePreview, QueueCounts, ReviewDecision, ReviewDetail, ReviewItem } from "@memex/types";
import { failed, ok } from "./api";
import type { Api, ApiFailure, ApiResult } from "./api";

export interface ReviewSeed {
  readonly items?: readonly ReviewDetail[];
  readonly autoCommit?: AutoCommitTable;
  readonly contradictions?: readonly Contradiction[];
  readonly inverse?: Readonly<Record<string, InversePreview>>;
  /** Who the fake thinks is deciding. */
  readonly decider?: string;
}

const http = (status: number, code: string, message = code): ApiFailure => ({ kind: "http", status, code, message, requestId: "req-1" });

export const changesetWith = (over: Partial<ChangeSet> & { readonly id: string }): ChangeSet => ({
  state: "pending",
  proposer: "an-agent",
  proposerIsMachine: true,
  requestedBy: null,
  rationale: "why",
  ops: [],
  originalOps: null,
  validators: [],
  risk: "review",
  proposedAt: epochMs(0),
  decidedAt: null,
  decidedBy: null,
  decisionReason: null,
  extractor: null,
  promptVersion: null,
  itemTypes: [],
  sourceIds: [],
  reverts: null,
  revertedBy: null,
  ...over,
});

export const detailWith = (changeset: ChangeSet, over: Partial<ReviewDetail> = {}): ReviewDetail => ({ changeset, verdict: null, evidence: [], combinations: [], needsAPerson: false, autoCommitBlock: null, ...over });

export const combinationWith = (over: Partial<AutoCommitEntry> = {}): AutoCommitEntry => ({
  id: "combo",
  extractor: "model",
  promptVersion: "p1",
  itemType: "claim",
  correct: 0,
  incorrect: 0,
  totalCorrect: 0,
  totalIncorrect: 0,
  precision: null,
  wilsonBound: 0,
  on: false,
  history: [],
  ...over,
});

type Answer = <T>(name: string, value: () => ApiResult<T>) => Promise<ApiResult<T>>;
type ReviewApi = Pick<Api, "listReviewQueue" | "getQueueCounts" | "getReviewItem" | "decide" | "decideBatch" | "listChangesets" | "getChangeset" | "getInverse" | "revertChangeset" | "listContradictions" | "listAutoCommit" | "disableAutoCommit">;

/** The review routes of the fake server: one decision per item, as the real one, and a second person's is a conflict that names the first. */
export const createReviewFake = (answer: Answer, seed: ReviewSeed = {}): ReviewApi => {
  const decider = seed.decider ?? "pat";
  const rows = new Map((seed.items ?? []).map((d) => [d.changeset.id, d]));
  const entries = new Map((seed.autoCommit?.entries ?? []).map((e) => [e.id, e]));
  const apply = (decision: ReviewDecision): ApiResult<ChangeSet> => {
    const row = rows.get(decision.id);
    if (row === undefined) return failed(http(404, "not_found"));
    const { changeset } = row;
    if (changeset.decidedBy !== null) {
      if (changeset.decidedBy === decider) return ok(changeset);
      return failed(http(409, "conflict", `${changeset.decidedBy} already decided this`));
    }
    if (decision.kind === "reject" && decision.reason.trim() === "") return failed(http(400, "invalid", "a rejection needs a reason"));
    const next: ChangeSet = {
      ...changeset,
      state: decision.kind === "reject" ? "rejected" : "committed",
      decidedBy: decider,
      decisionReason: decision.reason,
      ...(decision.kind === "edit_approve" ? { ops: decision.ops, originalOps: changeset.ops } : {}),
    };
    rows.set(decision.id, { ...row, changeset: next });
    return ok(next);
  };
  const pending = (): readonly ReviewItem[] => [...rows.values()].filter((d) => d.changeset.state === "pending");
  return {
    listReviewQueue: () => answer("listReviewQueue", () => ok({ items: pending(), nextCursor: null })),
    getQueueCounts: () => answer("getQueueCounts", (): ApiResult<QueueCounts> => ok({ pending: pending().length, groups: [] })),
    getReviewItem: (_vault, id) => answer("getReviewItem", () => {
      const row = rows.get(id);
      return row === undefined ? failed(http(404, "not_found")) : ok(row);
    }),
    decide: (_vault, decision) => answer("decide", () => apply(decision)),
    decideBatch: (_vault, decisions) => answer("decideBatch", () => {
      if (decisions.length > 200) return failed(http(400, "invalid", "at most 200 decisions at once"));
      return ok(decisions.map((d) => {
        const one = apply(d);
        if (one.ok) return { id: d.id, changeset: one.value, errorCode: null, errorMessage: null };
        return { id: d.id, changeset: null, errorCode: one.failure.kind === "http" ? one.failure.code : "failed", errorMessage: one.failure.message };
      }));
    }),
    listChangesets: (_vault, state) => answer("listChangesets", () => ok({ items: [...rows.values()].map((d) => d.changeset).filter((c) => state === "" || c.state === state), nextCursor: null })),
    getChangeset: (_vault, id) => answer("getChangeset", () => {
      const row = rows.get(id);
      return row === undefined ? failed(http(404, "not_found")) : ok(row.changeset);
    }),
    getInverse: (_vault, id) => answer("getInverse", () => {
      const inverse = seed.inverse?.[id];
      return inverse === undefined ? failed(http(404, "not_found")) : ok(inverse);
    }),
    revertChangeset: (_vault, id, reason) => answer("revertChangeset", () => {
      const row = rows.get(id);
      if (row === undefined) return failed(http(404, "not_found"));
      if (row.changeset.state !== "committed") return failed(http(409, "not_reversible", "only a committed change can be reverted"));
      const revert = { ...row.changeset, id: `revert-${id}`, reverts: id, decidedBy: decider, decisionReason: reason };
      rows.set(id, { ...row, changeset: { ...row.changeset, revertedBy: revert.id } });
      return ok(revert);
    }),
    listContradictions: () => answer("listContradictions", () => ok({ items: seed.contradictions ?? [], nextCursor: null })),
    listAutoCommit: () => answer("listAutoCommit", () => ok({ threshold: seed.autoCommit?.threshold ?? 0.95, entries: [...entries.values()], nextCursor: null })),
    disableAutoCommit: (_vault, combination, reason) => answer("disableAutoCommit", () => {
      const entry = entries.get(combination);
      if (entry === undefined) return failed(http(404, "not_found"));
      const next: AutoCommitEntry = { ...entry, on: false, correct: 0, incorrect: 0, precision: null, wilsonBound: 0, history: [...entry.history, { at: epochMs(1), by: decider, kind: "disabled", reason, correct: entry.correct, incorrect: entry.incorrect }] };
      entries.set(combination, next);
      return ok(next);
    }),
  };
};
