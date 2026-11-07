import type { ApiFailure, ApiResult, Engine } from "@memex/core";
import { MutationCache, QueryCache, QueryClient } from "@tanstack/react-query";

/** TanStack Query expects a thrown error, and the engine returns failures as data. This is the crossing, both ways. */
export class FailureError extends Error {
  readonly failure: ApiFailure;
  constructor(failure: ApiFailure) {
    super(failure.message);
    this.failure = failure;
  }
}

export const unwrap = <T>(result: ApiResult<T>): T => {
  if (!result.ok) throw new FailureError(result.failure);
  return result.value;
};

const failureOf = (error: unknown): ApiFailure | null => (error instanceof FailureError ? error.failure : null);

export const createQueryClient = (engine: Engine): QueryClient =>
  new QueryClient({
    queryCache: new QueryCache({
      onError: (error) => {
        const failure = failureOf(error);
        if (failure !== null) engine.reportFailure(failure, "read");
      },
    }),
    mutationCache: new MutationCache({
      onError: (error) => {
        const failure = failureOf(error);
        if (failure !== null) engine.reportFailure(failure, "action");
      },
    }),
    defaultOptions: {
      queries: {
        // The engine decides what's offline, by reaching the server, rather than the browser's less reliable idea of it.
        networkMode: "always",
        // A failed read shows its state at once. The banner's recovery refetches, and each error state has a retry.
        retry: false,
        refetchOnWindowFocus: false,
        staleTime: 30_000,
      },
      mutations: { networkMode: "always" },
    },
  });

export const QUERY_KEYS = {
  vaults: ["vaults"] as const,
  vault: (id: string) => ["vault", id] as const,
  objects: (id: string) => ["objects", id] as const,
  /** Everything about one vault's pages, so a write can refresh all of it. */
  wiki: (vault: string) => ["wiki", vault] as const,
  wikiPages: (vault: string) => ["wiki", vault, "pages"] as const,
  wikiPage: (vault: string, id: string, revision: number | null) => ["wiki", vault, "page", id, revision] as const,
  wikiBacklinks: (vault: string, id: string) => ["wiki", vault, "backlinks", id] as const,
  wikiLinks: (vault: string, id: string) => ["wiki", vault, "links", id] as const,
  wikiUnresolved: (vault: string) => ["wiki", vault, "unresolved"] as const,
  wikiRevisions: (vault: string, id: string) => ["wiki", vault, "revisions", id] as const,
  wikiDiff: (vault: string, id: string, from: number, to: number) => ["wiki", vault, "diff", id, from, to] as const,
  wikiSlug: (vault: string, slug: string) => ["wiki", vault, "slug", slug] as const,
  wikiClaim: (vault: string, id: string) => ["wiki", vault, "claim", id] as const,
  // A changeset's author never changes, so this one can stay cached across writes.
  wikiAttribution: (vault: string, changeset: string) => ["attribution", vault, changeset] as const,
};
