import {
  changeOriginFromWire,
  citingClaimsFromWire,
  claimLineageFromWire,
  derivationPageFromWire,
  failureFromWire,
  flaggedPagesFromWire,
  identityFromWire,
  lineageStepFromWire,
  mediaFromWire,
  objectsFromWire,
  pageListFromWire,
  pageWordsFromWire,
  proposalOfTrust,
  renditionFromWire,
  segmentFromWire,
  segmentPageFromWire,
  sourceFromWire,
  sourcePageFromWire,
  transcriptFromWire,
  vaultFromWire,
  vaultPageFromWire,
  wikiAttributionFromWire,
  wikiBacklinkPageFromWire,
  wikiDiffFromWire,
  wikiOutgoingPageFromWire,
  wikiPageFromWire,
  wikiRedirectFromWire,
  wikiRevisionPageFromWire,
  wikiSummaryPageFromWire,
  wikiUnresolvedPageFromWire,
  wikiWriteFromWire,
  autoCommitFromWire,
  batchFromWire,
  changesetFromWire,
  changesetPageFromWire,
  contradictionPageFromWire,
  inverseFromWire,
  queueCountsFromWire,
  reviewDetailFromWire,
  reviewFilterToWire,
  reviewPageFromWire,
} from "@memex/api";
import type { paths } from "@memex/api";
import { failed, ok } from "@memex/core";
import type { Api, ApiResult } from "@memex/core";
import createClient from "openapi-fetch";

/** The server wants this on anything a cookie would authorize. Its presence is the defence, so the value doesn't matter. */
const CSRF_HEADERS = { "x-memex-csrf": "1" };

/** The palette's first look at a vault: the API's largest page of each kind. */
const OBJECT_PAGE = { limit: 200 };
const page = (cursor: string | null) => ({ limit: 200, ...(cursor === null ? {} : { cursor }) });

interface Answer<D> {
  readonly data?: D;
  readonly error?: unknown;
  readonly response: Response;
}

/** No exception leaves this function. Everything becomes an `ApiResult`, so callers handle failures as data. */
const attempt = async <D, T>(run: () => Promise<Answer<D>>, translate: (data: D) => T): Promise<ApiResult<T>> => {
  try {
    const { data, error, response } = await run();
    if (!response.ok) return failed(failureFromWire(response.status, error, response.headers));
    return ok(translate(data as D));
  } catch (e) {
    // A body that doesn't parse is an answer, such as an HTML page, and anything else from fetch means no answer.
    if (e instanceof SyntaxError) return failed({ kind: "protocol", status: 200, message: "The server answered with something that isn't the API." });
    return failed({ kind: "unreachable", message: e instanceof Error ? e.message : "no answer" });
  }
};

/** `apiBaseUrl` is a same-origin path, checked by `settingsFrom`, so the browser's cookie and the CSP both apply. */
export const createFetchApi = ({ apiBaseUrl }: { readonly apiBaseUrl: string }): Api => {
  // The generated paths already start with /api/v1.
  const client = createClient<paths>({ baseUrl: apiBaseUrl.replace(/\/api\/v1$/, ""), headers: CSRF_HEADERS });
  const none = (): null => null;
  return {
    whoami: () => attempt(() => client.GET("/api/v1/whoami"), identityFromWire),
    signIn: (token) => attempt(() => client.POST("/api/v1/session", { body: { token } }), identityFromWire),
    signOut: () => attempt(() => client.DELETE("/api/v1/session"), none),
    listVaults: (cursor) =>
      attempt(() => client.GET("/api/v1/vaults", { params: { query: { limit: 200, ...(cursor === null ? {} : { cursor }) } } }), vaultPageFromWire),
    getVault: (id) => attempt(() => client.GET("/api/v1/vaults/{id}", { params: { path: { id } } }), vaultFromWire),
    listObjects: async (id) => {
      const params = { params: { path: { id }, query: OBJECT_PAGE } };
      const pass = <T>(value: T) => value;
      const [sources, pages, entityTypes] = await Promise.all([
        attempt(() => client.GET("/api/v1/vaults/{id}/sources", params), pass),
        attempt(() => client.GET("/api/v1/vaults/{id}/pages", params), pass),
        attempt(() => client.GET("/api/v1/vaults/{id}/entity-types", params), pass),
      ]);
      if (!sources.ok) return failed(sources.failure);
      if (!pages.ok) return failed(pages.failure);
      if (!entityTypes.ok) return failed(entityTypes.failure);
      return ok(objectsFromWire({ sources: sources.value, pages: pages.value, entityTypes: entityTypes.value }));
    },
    listWikiPages: (vault, cursor) => attempt(() => client.GET("/api/v1/vaults/{id}/pages", { params: { path: { id: vault }, query: page(cursor) } }), wikiSummaryPageFromWire),
    getWikiPage: (vault, id, revision) =>
      attempt(() => client.GET("/api/v1/vaults/{id}/pages/{pid}", { params: { path: { id: vault, pid: id }, query: revision === null ? {} : { revision } } }), wikiPageFromWire),
    resolveWikiSlug: (vault, slug) => attempt(() => client.GET("/api/v1/vaults/{id}/pages/by-slug/{slug}", { params: { path: { id: vault, slug } } }), wikiRedirectFromWire),
    listWikiBacklinks: (vault, id, cursor) =>
      attempt(() => client.GET("/api/v1/vaults/{id}/pages/{pid}/backlinks", { params: { path: { id: vault, pid: id }, query: page(cursor) } }), wikiBacklinkPageFromWire),
    listWikiLinks: (vault, id, cursor) =>
      attempt(() => client.GET("/api/v1/vaults/{id}/pages/{pid}/links", { params: { path: { id: vault, pid: id }, query: page(cursor) } }), wikiOutgoingPageFromWire),
    listWikiUnresolved: (vault, cursor) =>
      attempt(() => client.GET("/api/v1/vaults/{id}/pages/unresolved-links", { params: { path: { id: vault }, query: page(cursor) } }), wikiUnresolvedPageFromWire),
    listWikiRevisions: (vault, id, cursor) =>
      attempt(() => client.GET("/api/v1/vaults/{id}/pages/{pid}/revisions", { params: { path: { id: vault, pid: id }, query: page(cursor) } }), wikiRevisionPageFromWire),
    getWikiDiff: (vault, id, from, to) =>
      attempt(() => client.GET("/api/v1/vaults/{id}/pages/{pid}/diff", { params: { path: { id: vault, pid: id }, query: { from, to } } }), wikiDiffFromWire),
    getWikiAttribution: (vault, changesetId) => attempt(() => client.GET("/api/v1/vaults/{id}/changesets/{cs}", { params: { path: { id: vault, cs: changesetId } } }), wikiAttributionFromWire),
    exportWikiPage: (vault, id) =>
      attempt(() => client.GET("/api/v1/vaults/{id}/pages/{pid}/export", { params: { path: { id: vault, pid: id } }, parseAs: "text" }), (text) => String(text)),
    createWikiPage: (vault, input) =>
      attempt(
        () => client.POST("/api/v1/vaults/{id}/pages", { params: { path: { id: vault } }, body: { title: input.title, body: input.body, rationale: input.rationale, idempotency_key: input.idempotencyKey } }),
        (r) => ({ pageId: r.page_id, write: wikiWriteFromWire(r.result) }),
      ),
    editWikiPage: (vault, id, input) =>
      attempt(
        () => client.POST("/api/v1/vaults/{id}/pages/{pid}/edit", { params: { path: { id: vault, pid: id } }, body: { base_version: input.baseVersion, body: input.body, rationale: input.rationale, idempotency_key: input.idempotencyKey } }),
        wikiWriteFromWire,
      ),
    renameWikiPage: (vault, id, input) =>
      attempt(
        () => client.POST("/api/v1/vaults/{id}/pages/{pid}/rename", { params: { path: { id: vault, pid: id } }, body: { base_version: input.baseVersion, title: input.title, ...(input.slug === null ? {} : { slug: input.slug }), rationale: input.rationale, idempotency_key: input.idempotencyKey } }),
        wikiWriteFromWire,
      ),
    revertWikiPage: (vault, id, input) =>
      attempt(
        () => client.POST("/api/v1/vaults/{id}/pages/{pid}/revert", { params: { path: { id: vault, pid: id } }, body: { base_version: input.baseVersion, revision: input.revision, rationale: input.rationale, idempotency_key: input.idempotencyKey } }),
        wikiWriteFromWire,
      ),
    deleteWikiPage: (vault, id, input) =>
      attempt(
        () => client.POST("/api/v1/vaults/{id}/pages/{pid}/delete", { params: { path: { id: vault, pid: id } }, body: { base_version: input.baseVersion, rationale: input.rationale, idempotency_key: input.idempotencyKey } }),
        wikiWriteFromWire,
      ),
    importWikiPage: (vault, input) =>
      attempt(
        () => client.POST("/api/v1/vaults/{id}/pages/import", { params: { path: { id: vault } }, body: { markdown: input.markdown, ...(input.baseVersion === null ? {} : { base_version: input.baseVersion }), rationale: input.rationale, idempotency_key: input.idempotencyKey } }),
        (r) => ({ pageId: r.page_id, created: r.created, write: wikiWriteFromWire(r.result) }),
      ),
    searchClaimIds: (vault, q, limit) =>
      attempt(() => client.GET("/api/v1/vaults/{id}/search/lexical", { params: { path: { id: vault }, query: { q, limit, kind: "claim" } } }), (r) => r.hits.map((h) => h.id)),
    readClaim: (vault, id) =>
      attempt(() => client.GET("/api/v1/vaults/{id}/assertions/{aid}/lineage", { params: { path: { id: vault, aid: id } } }), (l) => {
        const step = lineageStepFromWire(id, l);
        return { claim: step.claim, supersededBy: step.supersededBy };
      }),
    listSources: (vault, cursor) => attempt(() => client.GET("/api/v1/vaults/{id}/sources", { params: { path: { id: vault }, query: page(cursor) } }), sourcePageFromWire),
    getSource: (vault, id) => attempt(() => client.GET("/api/v1/vaults/{id}/sources/{sid}", { params: { path: { id: vault, sid: id } } }), sourceFromWire),
    listSourceVersions: (vault, id) => attempt(() => client.GET("/api/v1/vaults/{id}/sources/{sid}/versions", { params: { path: { id: vault, sid: id } } }), (r) => r.items.map(sourceFromWire)),
    listRenditions: (vault, source) => attempt(() => client.GET("/api/v1/vaults/{id}/sources/{sid}/renditions", { params: { path: { id: vault, sid: source } } }), (r) => r.items.map(renditionFromWire)),
    listPageWords: (vault, rendition) => attempt(() => client.GET("/api/v1/vaults/{id}/renditions/{rid}", { params: { path: { id: vault, rid: rendition } } }), pageWordsFromWire),
    listSegments: (vault, rendition, cursor) => attempt(() => client.GET("/api/v1/vaults/{id}/renditions/{rid}/segments", { params: { path: { id: vault, rid: rendition }, query: page(cursor) } }), segmentPageFromWire),
    getSegment: (vault, id) => attempt(() => client.GET("/api/v1/vaults/{id}/segments/{seg}", { params: { path: { id: vault, seg: id } } }), segmentFromWire),
    listFlaggedPages: (vault, cursor) => attempt(() => client.GET("/api/v1/vaults/{id}/flagged-pages", { params: { path: { id: vault }, query: page(cursor) } }), flaggedPagesFromWire),
    listPages: (vault, source) => attempt(() => client.GET("/api/v1/vaults/{id}/sources/{sid}/pages", { params: { path: { id: vault, sid: source } } }), pageListFromWire),
    checkPageImage: (vault, source, pageIndex, dpi) =>
      attempt(() => client.GET("/api/v1/vaults/{id}/sources/{sid}/pages/{page}/image", { params: { path: { id: vault, sid: source, page: pageIndex }, query: { dpi } }, parseAs: "stream" }), none),
    listSourceClaims: (vault, source, cursor) => attempt(() => client.GET("/api/v1/vaults/{id}/sources/{sid}/claims", { params: { path: { id: vault, sid: source }, query: page(cursor) } }), citingClaimsFromWire),
    getLineage: (vault, claim) => attempt(() => client.GET("/api/v1/vaults/{id}/assertions/{aid}/lineage", { params: { path: { id: vault, aid: claim } } }), (l) => claimLineageFromWire(claim, l)),
    getChangeOrigin: (vault, changesetId) => attempt(() => client.GET("/api/v1/vaults/{id}/changesets/{cs}", { params: { path: { id: vault, cs: changesetId } } }), changeOriginFromWire),
    listDerivations: (vault, rendition, cursor) => attempt(() => client.GET("/api/v1/vaults/{id}/renditions/{rid}/derivations", { params: { path: { id: vault, rid: rendition }, query: page(cursor) } }), derivationPageFromWire),
    getMedia: (vault, source) => attempt(() => client.GET("/api/v1/vaults/{id}/sources/{sid}/media", { params: { path: { id: vault, sid: source } } }), mediaFromWire),
    getTranscript: (vault, source, { fromMs, cursor, words }) =>
      attempt(() => client.GET("/api/v1/vaults/{id}/sources/{sid}/transcript", { params: { path: { id: vault, sid: source }, query: { limit: 200, words, ...(fromMs === null ? {} : { from_ms: fromMs }), ...(cursor === null ? {} : { cursor }) } } }), transcriptFromWire),
    reprocessSource: (vault, source) => attempt(() => client.POST("/api/v1/vaults/{id}/sources/{sid}/reprocess", { params: { path: { id: vault, sid: source } } }), none),
    setSourceTrust: (vault, source, change) =>
      attempt(() => client.POST("/api/v1/vaults/{id}/proposals", { params: { path: { id: vault } }, body: { ops: [proposalOfTrust(source, change.baseVersion, change.tier) as never], rationale: change.rationale, idempotency_key: change.idempotencyKey } }), wikiWriteFromWire),
    listReviewQueue: (vault, filter, cursor) =>
      attempt(() => client.GET("/api/v1/vaults/{id}/review/queue", { params: { path: { id: vault }, query: { ...page(cursor), state: "pending", ...reviewFilterToWire(filter) } } }), reviewPageFromWire),
    getQueueCounts: (vault) => attempt(() => client.GET("/api/v1/vaults/{id}/review/counts", { params: { path: { id: vault } } }), queueCountsFromWire),
    getReviewItem: (vault, id) => attempt(() => client.GET("/api/v1/vaults/{id}/review/items/{cs}", { params: { path: { id: vault, cs: id } } }), reviewDetailFromWire),
    decide: (vault, decision) => {
      const path = { id: vault, cs: decision.id };
      if (decision.kind === "approve") return attempt(() => client.POST("/api/v1/vaults/{id}/changesets/{cs}/approve", { params: { path }, body: { reason: decision.reason } }), changesetFromWire);
      if (decision.kind === "reject") return attempt(() => client.POST("/api/v1/vaults/{id}/changesets/{cs}/reject", { params: { path }, body: { reason: decision.reason } }), changesetFromWire);
      return attempt(() => client.POST("/api/v1/vaults/{id}/review/items/{cs}/edit-approve", { params: { path }, body: { ops: decision.ops as never, reason: decision.reason } }), changesetFromWire);
    },
    decideBatch: (vault, decisions) =>
      attempt(
        () =>
          client.POST("/api/v1/vaults/{id}/review/batch", {
            params: { path: { id: vault } },
            body: { decisions: decisions.map((d) => ({ id: d.id, decision: d.kind, reason: d.reason, ...(d.kind === "edit_approve" ? { ops: d.ops as never } : {}) })) },
          }),
        batchFromWire,
      ),
    listChangesets: (vault, state, cursor) =>
      attempt(() => client.GET("/api/v1/vaults/{id}/changesets", { params: { path: { id: vault }, query: { ...page(cursor), ...(state === "" ? {} : { state }) } } }), changesetPageFromWire),
    getChangeset: (vault, id) => attempt(() => client.GET("/api/v1/vaults/{id}/changesets/{cs}", { params: { path: { id: vault, cs: id } } }), changesetFromWire),
    getInverse: (vault, id) => attempt(() => client.GET("/api/v1/vaults/{id}/changesets/{cs}/inverse", { params: { path: { id: vault, cs: id } } }), inverseFromWire),
    revertChangeset: (vault, id, reason) => attempt(() => client.POST("/api/v1/vaults/{id}/changesets/{cs}/revert", { params: { path: { id: vault, cs: id } }, body: { reason } }), changesetFromWire),
    listContradictions: (vault, cursor) => attempt(() => client.GET("/api/v1/vaults/{id}/contradictions", { params: { path: { id: vault }, query: page(cursor) } }), contradictionPageFromWire),
    listAutoCommit: (vault, cursor) =>
      attempt(() => client.GET("/api/v1/vaults/{id}/review/auto-commit", { params: { path: { id: vault }, query: page(cursor) } }), (r) => ({ threshold: r.threshold, entries: r.items.map(autoCommitFromWire), nextCursor: r.next_cursor ?? null })),
    disableAutoCommit: (vault, combination, reason) =>
      attempt(() => client.POST("/api/v1/vaults/{id}/review/auto-commit/{combination}/disable", { params: { path: { id: vault, combination } }, body: { reason } }), autoCommitFromWire),
    health: (timeoutMs) => attempt(() => client.GET("/api/v1/health", { signal: AbortSignal.timeout(timeoutMs) }), none),
  };
};
