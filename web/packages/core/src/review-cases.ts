import type { AutoCommitEntry, AutoCommitTable, BatchOutcome, ChangeSet, Contradiction, InversePreview, Page, QueueCounts, ReviewDecision, ReviewDetail, ReviewFilter, ReviewItem, VaultId } from "@memex/types";
import { failed, ok } from "./api";
import type { Api, ApiResult } from "./api";
import { batchesOf, decisionFailure } from "./review";
import type { DecisionOutcome } from "./review";

export type QueueFeedEvent =
  /** The stream opened, or came back. What was read before may have missed something, so read again. */
  | { readonly kind: "open" }
  /** Something that could move the queue happened. */
  | { readonly kind: "changed" }
  /** The server dropped frames, so read again. */
  | { readonly kind: "resync" }
  /** The stream is away and trying to come back. */
  | { readonly kind: "away" }
  /** It won't come back by itself, such as over the limit on streams or a session that ended. */
  | { readonly kind: "closed" };

/** The activity stream, as much as the queue needs of it. */
export interface QueueFeed {
  follow(vault: VaultId, listener: (event: QueueFeedEvent) => void): () => void;
}

export interface ReviewCases {
  listReviewQueue(vault: VaultId, filter: ReviewFilter, cursor: string | null): Promise<ApiResult<Page<ReviewItem>>>;
  getQueueCounts(vault: VaultId): Promise<ApiResult<QueueCounts>>;
  getReviewItem(vault: VaultId, id: string): Promise<ApiResult<ReviewDetail>>;
  decide(vault: VaultId, decision: ReviewDecision): Promise<DecisionOutcome>;
  decideBatch(vault: VaultId, decisions: readonly ReviewDecision[]): Promise<ApiResult<readonly BatchOutcome[]>>;
  listChangesets(vault: VaultId, state: string, cursor: string | null): Promise<ApiResult<Page<ChangeSet>>>;
  getChangeset(vault: VaultId, id: string): Promise<ApiResult<ChangeSet>>;
  getInverse(vault: VaultId, id: string): Promise<ApiResult<InversePreview>>;
  revertChangeset(vault: VaultId, id: string, reason: string): Promise<DecisionOutcome>;
  listContradictions(vault: VaultId, cursor: string | null): Promise<ApiResult<Page<Contradiction>>>;
  getAutoCommitTable(vault: VaultId): Promise<ApiResult<AutoCommitTable>>;
  disableAutoCommit(vault: VaultId, combination: string, reason: string): Promise<ApiResult<AutoCommitEntry>>;
  /** Returns the way to stop. With no feed it says so at once, and the screen reads by itself instead. */
  followQueue(vault: VaultId, listener: (event: QueueFeedEvent) => void): () => void;
}

interface ReviewDeps {
  readonly edges: { readonly api: Api; readonly queueFeed?: QueueFeed };
  readonly watched: <T>(call: Promise<ApiResult<T>>) => Promise<ApiResult<T>>;
}

/** A table of combinations can't run for ever: 20 pages of 200 is 4,000 rows. */
const MAX_TABLE_PAGES = 20;

const outcomeOf = (result: ApiResult<ChangeSet>): DecisionOutcome => (result.ok ? { kind: "done", changeset: result.value } : decisionFailure(result.failure));

export const createReviewCases = ({ edges, watched }: ReviewDeps): ReviewCases => {
  const { api, queueFeed } = edges;
  return {
    listReviewQueue: (vault, filter, cursor) => watched(api.listReviewQueue(vault, filter, cursor)),
    getQueueCounts: (vault) => watched(api.getQueueCounts(vault)),
    getReviewItem: (vault, id) => watched(api.getReviewItem(vault, id)),
    decide: async (vault, decision) => outcomeOf(await watched(api.decide(vault, decision))),
    decideBatch: async (vault, decisions) => {
      const all: BatchOutcome[] = [];
      for (const batch of batchesOf(decisions)) {
        const result = await watched(api.decideBatch(vault, batch));
        // What went through has gone through, and the refetch shows it. The failure says the rest didn't.
        if (!result.ok) return all.length === 0 ? failed(result.failure) : ok([...all, ...decisions.slice(all.length).map((d) => ({ id: d.id, changeset: null, errorCode: "not_sent", errorMessage: "This wasn't sent, as an earlier batch failed." }))]);
        all.push(...result.value);
      }
      return ok(all);
    },
    listChangesets: (vault, state, cursor) => watched(api.listChangesets(vault, state, cursor)),
    getChangeset: (vault, id) => watched(api.getChangeset(vault, id)),
    getInverse: (vault, id) => watched(api.getInverse(vault, id)),
    revertChangeset: async (vault, id, reason) => outcomeOf(await watched(api.revertChangeset(vault, id, reason))),
    listContradictions: (vault, cursor) => watched(api.listContradictions(vault, cursor)),
    getAutoCommitTable: async (vault) => {
      const entries: AutoCommitEntry[] = [];
      let threshold = 0;
      let cursor: string | null = null;
      for (let page = 0; page < MAX_TABLE_PAGES; page += 1) {
        const result: ApiResult<{ readonly threshold: number; readonly entries: readonly AutoCommitEntry[]; readonly nextCursor: string | null }> = await watched(api.listAutoCommit(vault, cursor));
        if (!result.ok) return failed(result.failure);
        threshold = result.value.threshold;
        entries.push(...result.value.entries);
        cursor = result.value.nextCursor;
        if (cursor === null) break;
      }
      return ok({ threshold, entries });
    },
    disableAutoCommit: (vault, combination, reason) => watched(api.disableAutoCommit(vault, combination, reason)),
    followQueue: (vault, listener) => {
      if (queueFeed === undefined) {
        listener({ kind: "closed" });
        return () => undefined;
      }
      return queueFeed.follow(vault, listener);
    },
  };
};
