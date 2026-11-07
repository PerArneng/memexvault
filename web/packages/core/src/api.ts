import type { ApiFailure, Identity, ObjectSummary, Page, Vault, VaultId } from "@memex/types";

export type { ApiFailure };

export type ApiResult<T> = { readonly ok: true; readonly value: T } | { readonly ok: false; readonly failure: ApiFailure };

export const ok = <T>(value: T): ApiResult<T> => ({ ok: true, value });
export const failed = <T>(failure: ApiFailure): ApiResult<T> => ({ ok: false, failure });

export interface Api {
  whoami(): Promise<ApiResult<Identity>>;
  signIn(token: string): Promise<ApiResult<Identity>>;
  signOut(): Promise<ApiResult<null>>;
  listVaults(cursor: string | null): Promise<ApiResult<Page<Vault>>>;
  getVault(id: VaultId): Promise<ApiResult<Vault>>;
  /** The first objects of each kind in a vault. */
  listObjects(id: VaultId): Promise<ApiResult<readonly ObjectSummary[]>>;
  /** A failure includes no answer within `timeoutMs`. */
  health(timeoutMs: number): Promise<ApiResult<null>>;
}

export const isUnauthenticated = (failure: ApiFailure): boolean => failure.kind === "http" && failure.status === 401;
export const isForbidden = (failure: ApiFailure): boolean => failure.kind === "http" && failure.status === 403;
export const isNotFound = (failure: ApiFailure): boolean => failure.kind === "http" && failure.status === 404;

/** A request id, if the server gave one, in the one form every screen shows it. */
export const requestIdText = (failure: ApiFailure): string | null => (failure.kind === "http" ? `request id: ${failure.requestId}` : null);
