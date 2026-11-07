import {
  failureFromWire,
  identityFromWire,
  lineageStepFromWire,
  objectsFromWire,
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
    health: (timeoutMs) => attempt(() => client.GET("/api/v1/health", { signal: AbortSignal.timeout(timeoutMs) }), none),
  };
};
