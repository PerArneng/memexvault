import type {
  Page,
  VaultId,
  WikiAttribution,
  WikiBacklink,
  WikiClaim,
  WikiClaimOption,
  WikiDiff,
  WikiOutgoingLink,
  WikiPage,
  WikiPageSummary,
  WikiRedirect,
  WikiRevision,
  WikiUnresolvedLink,
  WikiWrite,
} from "@memex/types";
import { failed, isNotFound, ok } from "./api";
import type { Api, ApiFailure, ApiResult } from "./api";
import type { Clock } from "./clock";
import { createDraftKeeper, draftKey } from "./draft";
import type { DraftKeeper, DraftStorage } from "./draft";

/** Fresh ids for idempotency keys. Randomness is an edge, as pure code has none. */
export interface IdSource {
  next(): string;
}

export interface WikiPageList {
  readonly pages: readonly WikiPageSummary[];
  /** There are more than can be listed in the browser, so "not in this list" doesn't mean "doesn't exist". */
  readonly truncated: boolean;
}

/** What a save became. Only `saved` and `pending` mean the server has the text, so only those let a draft go. */
export type SaveOutcome =
  | { readonly kind: "saved"; readonly changesetId: string | null }
  /** Governance holds it for review, so the page hasn't changed yet. */
  | { readonly kind: "pending"; readonly changesetId: string | null }
  | { readonly kind: "refused"; readonly reasons: readonly string[] }
  /** Someone else got there first. Nothing was written. `base` is what this edit started from, if that can still be read. */
  | { readonly kind: "conflict"; readonly mine: string; readonly theirs: WikiPage; readonly base: WikiPage | null }
  | { readonly kind: "failed"; readonly failure: ApiFailure };

export type WikiChange =
  | { readonly kind: "saved"; readonly changesetId: string | null }
  | { readonly kind: "pending"; readonly changesetId: string | null }
  | { readonly kind: "refused"; readonly reasons: readonly string[] }
  /** The page moved on, so this rename, revert or delete did nothing. It's on the person to look again. */
  | { readonly kind: "stale" }
  | { readonly kind: "failed"; readonly failure: ApiFailure };

export interface WikiCases {
  listWikiPages(vault: VaultId): Promise<ApiResult<WikiPageList>>;
  getWikiPage(vault: VaultId, id: string, revision: number | null): Promise<ApiResult<WikiPage>>;
  resolveWikiSlug(vault: VaultId, slug: string): Promise<ApiResult<WikiRedirect>>;
  listWikiBacklinks(vault: VaultId, id: string): Promise<ApiResult<readonly WikiBacklink[]>>;
  listWikiLinks(vault: VaultId, id: string): Promise<ApiResult<readonly WikiOutgoingLink[]>>;
  listWikiUnresolved(vault: VaultId): Promise<ApiResult<readonly WikiUnresolvedLink[]>>;
  listWikiRevisions(vault: VaultId, id: string): Promise<ApiResult<readonly WikiRevision[]>>;
  getWikiDiff(vault: VaultId, id: string, from: number, to: number): Promise<ApiResult<WikiDiff>>;
  getWikiAttribution(vault: VaultId, changesetId: string): Promise<ApiResult<WikiAttribution>>;
  exportWikiPage(vault: VaultId, id: string): Promise<ApiResult<string>>;

  /** What a claim reference shows: following replacements to what stands, as the page route does. An id that leads nowhere is `missing`, not a failure. */
  lookupClaim(vault: VaultId, id: string): Promise<ApiResult<WikiClaim>>;
  suggestClaims(vault: VaultId, query: string): Promise<ApiResult<readonly WikiClaimOption[]>>;

  draftKeeper(vault: VaultId, page: string): DraftKeeper;
  createWikiPage(vault: VaultId, input: { readonly title: string; readonly body: string; readonly rationale: string }): Promise<SaveOutcome & { readonly pageId?: string }>;
  saveWikiPage(vault: VaultId, id: string, input: { readonly baseVersion: number; readonly body: string; readonly rationale: string }): Promise<SaveOutcome>;
  renameWikiPage(vault: VaultId, id: string, input: { readonly baseVersion: number; readonly title: string; readonly slug: string | null; readonly rationale: string }): Promise<WikiChange>;
  revertWikiPage(vault: VaultId, id: string, input: { readonly baseVersion: number; readonly revision: number; readonly rationale: string }): Promise<WikiChange>;
  deleteWikiPage(vault: VaultId, id: string, input: { readonly baseVersion: number; readonly rationale: string }): Promise<WikiChange>;
  importWikiPage(vault: VaultId, input: { readonly markdown: string; readonly baseVersion: number | null; readonly rationale: string }): Promise<WikiChange & { readonly pageId?: string }>;
}

/** The cursors of a list can't run for ever: 50 pages of 200 is 10,000 rows, which is also as many as a browser list should hold. */
const MAX_LIST_PAGES = 50;
/** How far a chain of replaced claims is followed, as on the server. */
const MAX_SUPERSEDED = 64;
const MAX_CLAIM_SUGGESTIONS = 8;

const isStale = (failure: ApiFailure): boolean => failure.kind === "http" && failure.code === "stale_version";

const outcomeOf = (write: WikiWrite): { readonly kind: "saved" | "pending"; readonly changesetId: string | null } | { readonly kind: "refused"; readonly reasons: readonly string[] } => {
  if (write.state === "committed") return { kind: "saved", changesetId: write.changesetId };
  if (write.state === "pending") return { kind: "pending", changesetId: write.changesetId };
  return { kind: "refused", reasons: write.refusals };
};

interface WikiDeps {
  readonly edges: { readonly api: Api; readonly clock: Clock; readonly drafts: DraftStorage; readonly ids: IdSource };
  readonly watched: <T>(call: Promise<ApiResult<T>>) => Promise<ApiResult<T>>;
  readonly principal: () => string;
}

export const createWikiCases = ({ edges, watched, principal }: WikiDeps): WikiCases => {
  const { api, clock, drafts, ids } = edges;

  /**
   * A write with no answer may still have landed, so retrying the same write has to reuse its key: the server then replays the first result,
   * rather than reporting a conflict with the edit that was its own. A key lives only until an answer arrives.
   */
  const keys = new Map<string, string>();
  const keyed = async <T>(what: string, call: (key: string) => Promise<ApiResult<T>>): Promise<ApiResult<T>> => {
    const key = keys.get(what) ?? ids.next();
    keys.set(what, key);
    const result = await watched(call(key));
    if (result.ok || result.failure.kind !== "unreachable") keys.delete(what);
    return result;
  };

  const all = async <T>(fetchPage: (cursor: string | null) => Promise<ApiResult<Page<T>>>): Promise<ApiResult<{ readonly items: readonly T[]; readonly truncated: boolean }>> => {
    const items: T[] = [];
    let cursor: string | null = null;
    for (let page = 0; page < MAX_LIST_PAGES; page += 1) {
      const result: ApiResult<Page<T>> = await watched(fetchPage(cursor));
      if (!result.ok) return failed(result.failure);
      items.push(...result.value.items);
      cursor = result.value.nextCursor;
      if (cursor === null) return ok({ items, truncated: false });
    }
    return ok({ items, truncated: true });
  };
  const itemsOf = async <T>(fetchPage: (cursor: string | null) => Promise<ApiResult<Page<T>>>): Promise<ApiResult<readonly T[]>> => {
    const result = await all(fetchPage);
    return result.ok ? ok(result.value.items) : failed(result.failure);
  };

  const missing = (id: string): WikiClaim => ({ claimId: id, status: "missing", currentClaimId: null, subject: null, predicate: null, value: null, epistemic: null, citations: [] });

  const lookupClaim: WikiCases["lookupClaim"] = async (vault, id) => {
    let at = id;
    for (let hop = 0; hop < MAX_SUPERSEDED; hop += 1) {
      const step = await watched(api.readClaim(vault, at));
      if (!step.ok) return isNotFound(step.failure) ? ok(missing(id)) : failed(step.failure);
      if (step.value.supersededBy === null) {
        return ok({ ...step.value.claim, claimId: id, currentClaimId: at, status: at === id ? "current" : "superseded" });
      }
      at = step.value.supersededBy;
    }
    return ok(missing(id));
  };

  const conflictFor = async (vault: VaultId, id: string, mine: string, baseVersion: number): Promise<SaveOutcome> => {
    const theirs = await watched(api.getWikiPage(vault, id, null));
    if (!theirs.ok) return { kind: "failed", failure: theirs.failure };
    // Identical text means whoever got there first wrote this, which is often this person in another tab. There's nothing to choose.
    if (theirs.value.body === mine) return { kind: "saved", changesetId: null };
    const base = await watched(api.getWikiPage(vault, id, baseVersion));
    return { kind: "conflict", mine, theirs: theirs.value, base: base.ok ? base.value : null };
  };

  const changeOf = (result: ApiResult<WikiWrite>): WikiChange => {
    if (result.ok) return outcomeOf(result.value);
    return isStale(result.failure) ? { kind: "stale" } : { kind: "failed", failure: result.failure };
  };

  return {
    listWikiPages: async (vault) => {
      const result = await all((cursor) => api.listWikiPages(vault, cursor));
      return result.ok ? ok({ pages: result.value.items, truncated: result.value.truncated }) : failed(result.failure);
    },
    getWikiPage: (vault, id, revision) => watched(api.getWikiPage(vault, id, revision)),
    resolveWikiSlug: (vault, slug) => watched(api.resolveWikiSlug(vault, slug)),
    listWikiBacklinks: (vault, id) => itemsOf((cursor) => api.listWikiBacklinks(vault, id, cursor)),
    listWikiLinks: (vault, id) => itemsOf((cursor) => api.listWikiLinks(vault, id, cursor)),
    listWikiUnresolved: (vault) => itemsOf((cursor) => api.listWikiUnresolved(vault, cursor)),
    listWikiRevisions: (vault, id) => itemsOf((cursor) => api.listWikiRevisions(vault, id, cursor)),
    getWikiDiff: (vault, id, from, to) => watched(api.getWikiDiff(vault, id, from, to)),
    getWikiAttribution: (vault, changesetId) => watched(api.getWikiAttribution(vault, changesetId)),
    exportWikiPage: (vault, id) => watched(api.exportWikiPage(vault, id)),

    lookupClaim,
    suggestClaims: async (vault, query) => {
      const found = await watched(api.searchClaimIds(vault, query, MAX_CLAIM_SUGGESTIONS));
      if (!found.ok) return failed(found.failure);
      const claims = await Promise.all(found.value.map((id) => lookupClaim(vault, id)));
      return ok(
        claims.flatMap((result) => {
          // A current claim only: a new reference to a replaced one would be stale from the start.
          if (!result.ok || result.value.status !== "current") return [];
          const { claimId, subject, predicate, value } = result.value;
          return [{ id: claimId, label: [subject, predicate, value].filter((part) => part !== null).join(" ") }];
        }),
      );
    },

    draftKeeper: (vault, page) => createDraftKeeper({ clock, storage: drafts, key: draftKey(principal(), vault, page) }),

    createWikiPage: async (vault, { title, body, rationale }) => {
      const result = await keyed(`create\0${vault}\0${title}\0${body}`, (idempotencyKey) => api.createWikiPage(vault, { title, body, rationale, idempotencyKey }));
      if (!result.ok) return { kind: "failed", failure: result.failure };
      return { ...outcomeOf(result.value.write), pageId: result.value.pageId };
    },
    saveWikiPage: async (vault, id, { baseVersion, body, rationale }) => {
      const result = await keyed(`edit\0${vault}\0${id}\0${String(baseVersion)}\0${body}`, (idempotencyKey) => api.editWikiPage(vault, id, { baseVersion, body, rationale, idempotencyKey }));
      if (result.ok) return outcomeOf(result.value);
      return isStale(result.failure) ? conflictFor(vault, id, body, baseVersion) : { kind: "failed", failure: result.failure };
    },
    renameWikiPage: async (vault, id, { baseVersion, title, slug, rationale }) =>
      changeOf(await keyed(`rename\0${vault}\0${id}\0${String(baseVersion)}\0${title}\0${slug ?? ""}`, (idempotencyKey) => api.renameWikiPage(vault, id, { baseVersion, title, slug, rationale, idempotencyKey }))),
    revertWikiPage: async (vault, id, { baseVersion, revision, rationale }) =>
      changeOf(await keyed(`revert\0${vault}\0${id}\0${String(baseVersion)}\0${String(revision)}`, (idempotencyKey) => api.revertWikiPage(vault, id, { baseVersion, revision, rationale, idempotencyKey }))),
    deleteWikiPage: async (vault, id, { baseVersion, rationale }) =>
      changeOf(await keyed(`delete\0${vault}\0${id}\0${String(baseVersion)}`, (idempotencyKey) => api.deleteWikiPage(vault, id, { baseVersion, rationale, idempotencyKey }))),
    importWikiPage: async (vault, { markdown, baseVersion, rationale }) => {
      const result = await keyed(`import\0${vault}\0${String(baseVersion)}\0${markdown}`, (idempotencyKey) => api.importWikiPage(vault, { markdown, baseVersion, rationale, idempotencyKey }));
      if (result.ok) return { ...outcomeOf(result.value.write), pageId: result.value.pageId };
      return isStale(result.failure) ? { kind: "stale" } : { kind: "failed", failure: result.failure };
    },
  };
};
