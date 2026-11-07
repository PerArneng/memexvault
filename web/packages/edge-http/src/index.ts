import { failureFromWire, identityFromWire, objectsFromWire, vaultFromWire, vaultPageFromWire } from "@memex/api";
import type { paths } from "@memex/api";
import { failed, ok } from "@memex/core";
import type { Api, ApiResult } from "@memex/core";
import createClient from "openapi-fetch";

/** The server wants this on anything a cookie would authorize. Its presence is the defence, so the value doesn't matter. */
const CSRF_HEADERS = { "x-memex-csrf": "1" };

/** The palette's first look at a vault: the API's largest page of each kind. */
const OBJECT_PAGE = { limit: 200 };

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
    health: (timeoutMs) => attempt(() => client.GET("/api/v1/health", { signal: AbortSignal.timeout(timeoutMs) }), none),
  };
};
