import { documentPath, isForbidden, isNotFound, parseDocumentRoute, requestIdText } from "@memex/core";
import type { DocumentRoute, SourceViewRoute } from "@memex/core";
import type { VaultId } from "@memex/types";
import { EmptyState } from "@memex/ui";
import { useLocation } from "@tanstack/react-router";
import { useCallback } from "react";
import type { UseQueryResult } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { FailureError } from "../query";
import { QueryView } from "../shell/query-view";
import { AppLink } from "../wiki/wiki-common";

export const routeFrom = (rest: readonly string[], search: string): DocumentRoute => {
  const query = new URLSearchParams(search);
  return parseDocumentRoute(rest, (name) => query.get(name));
};

export const useDocumentRoute = (rest: readonly string[]): DocumentRoute => routeFrom(rest, useLocation().searchStr);

export const sourceRoute = (id: string, over: Partial<SourceViewRoute> = {}): SourceViewRoute => ({
  kind: "source", id, tab: "view", page: null, zoom: null, claim: null, evidence: null, fromMs: null, toMs: null, rendition: null, segment: null, ...over,
});

export const useSourcePath = (vault: VaultId) => useCallback((id: string, over: Partial<SourceViewRoute> = {}) => documentPath(vault, sourceRoute(id, over)), [vault]);

/** A source that's gone looks the same whether it was erased, never existed, or isn't this token's to see. The server doesn't say which, and neither do we. */
export const GoneSource = ({ vault, detail }: { readonly vault: VaultId; readonly detail?: string | null }) => (
  <div data-source-gone={true}>
    <EmptyState title="This document isn't available.">
      It may have been erased, it may never have existed, or your token may have no access to it. {detail} <AppLink to={documentPath(vault, { kind: "list", filter: { text: "", tier: "", kind: "", flagged: false } })}>Go to Documents</AppLink>
    </EmptyState>
  </div>
);

/** `QueryView`, but a 404 on a source reads as above. */
export const SourceQueryView = <T,>({ vault, query, label, children }: { readonly vault: VaultId; readonly query: UseQueryResult<T>; readonly label: string; readonly children: (data: T) => ReactNode }) => {
  const failure = query.error instanceof FailureError ? query.error.failure : null;
  if (failure !== null && isNotFound(failure) && query.data === undefined) return <GoneSource vault={vault} detail={requestIdText(failure)} />;
  return <QueryView query={query} label={label}>{children}</QueryView>;
};

export const isForbiddenError = (error: unknown): boolean => error instanceof FailureError && isForbidden(error.failure);
export const failureCode = (error: unknown): string | null => (error instanceof FailureError && error.failure.kind === "http" ? error.failure.code : null);
export const failureStatus = (error: unknown): number | null => (error instanceof FailureError && error.failure.kind === "http" ? error.failure.status : null);
