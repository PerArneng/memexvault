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
};
