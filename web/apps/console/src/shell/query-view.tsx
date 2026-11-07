import { isForbidden, isNotFound, requestIdText } from "@memex/core";
import { EmptyState, ErrorState, Skeleton } from "@memex/ui";
import type { UseQueryResult } from "@tanstack/react-query";
import type { ReactNode } from "react";
import { useConnectivity } from "../engine-context";
import { FailureError } from "../query";

export interface QueryViewProps<T> {
  readonly query: UseQueryResult<T>;
  /** What the loading state stands in for. */
  readonly label: string;
  readonly isEmpty?: (data: T) => boolean;
  readonly empty?: ReactNode;
  readonly children: (data: T) => ReactNode;
}

const Loading = ({ label }: { readonly label: string }) => (
  <div className="flex flex-col gap-2">
    <p role="status" className="sr-only">Loading {label}</p>
    <Skeleton className="h-5 w-1/3" />
    <Skeleton className="h-24 w-full" />
  </div>
);

export const OfflineState = ({ label }: { readonly label: string }) => (
  <EmptyState title={`Can't load ${label} while the server is out of reach.`}>It will load by itself when the connection returns.</EmptyState>
);

export const PermissionMessage = ({ title, children }: { readonly title: string; readonly children?: ReactNode }) => (
  <div role="alert" className="flex flex-col gap-2 rounded-lg border border-warning bg-warning-surface p-4">
    <p className="font-medium text-warning">{title}</p>
    {children !== undefined && <div className="text-sm text-text-muted">{children}</div>}
  </div>
);

/**
 * The one way every screen shows loading, empty, error and offline, so they read the same everywhere.
 * Data already on screen stays while a refetch fails, since the banner or toast says what's wrong.
 */
export const QueryView = <T,>({ query, label, isEmpty, empty, children }: QueryViewProps<T>) => {
  const connectivity = useConnectivity();
  if (query.data !== undefined) {
    return isEmpty?.(query.data) === true ? <>{empty ?? <EmptyState title={`No ${label} yet.`} />}</> : <>{children(query.data)}</>;
  }
  if (query.isPending) return connectivity.status === "offline" ? <OfflineState label={label} /> : <Loading label={label} />;
  const failure = query.error instanceof FailureError ? query.error.failure : null;
  if (failure === null) return <ErrorState title={`Couldn't load ${label}.`} onRetry={() => void query.refetch()} />;
  if (failure.kind === "unreachable" || connectivity.status === "offline") return <OfflineState label={label} />;
  const detail = requestIdText(failure);
  if (isForbidden(failure)) return <PermissionMessage title={`You don't have permission to see ${label}.`}>{detail}</PermissionMessage>;
  if (isNotFound(failure)) return <EmptyState title={`Couldn't find ${label}.`}>It doesn't exist, or your token has no access to it. {detail}</EmptyState>;
  return <ErrorState title={failure.message} {...(detail === null ? {} : { detail })} onRetry={() => void query.refetch()} />;
};
