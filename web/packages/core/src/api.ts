import type {
  ApiFailure,
  Identity,
  ObjectSummary,
  Page,
  Vault,
  VaultId,
  WikiAttribution,
  WikiBacklink,
  WikiClaim,
  WikiDiff,
  WikiOutgoingLink,
  WikiPage,
  WikiPageSummary,
  WikiRedirect,
  WikiRevision,
  WikiUnresolvedLink,
  WikiWrite,
} from "@memex/types";

export type { ApiFailure };

export type ApiResult<T> = { readonly ok: true; readonly value: T } | { readonly ok: false; readonly failure: ApiFailure };

export const ok = <T>(value: T): ApiResult<T> => ({ ok: true, value });
export const failed = <T>(failure: ApiFailure): ApiResult<T> => ({ ok: false, failure });

export interface WikiWriteBase {
  readonly baseVersion: number;
  readonly rationale: string;
  readonly idempotencyKey: string;
}
export interface NewWikiPage {
  readonly title: string;
  readonly body: string;
  readonly rationale: string;
  readonly idempotencyKey: string;
}
export interface WikiEdit extends WikiWriteBase {
  readonly body: string;
}
export interface WikiRename extends WikiWriteBase {
  readonly title: string;
  readonly slug: string | null;
}
export interface WikiRevert extends WikiWriteBase {
  readonly revision: number;
}
export interface WikiImport {
  readonly markdown: string;
  readonly baseVersion: number | null;
  readonly rationale: string;
  readonly idempotencyKey: string;
}

export interface Api {
  whoami(): Promise<ApiResult<Identity>>;
  signIn(token: string): Promise<ApiResult<Identity>>;
  signOut(): Promise<ApiResult<null>>;
  listVaults(cursor: string | null): Promise<ApiResult<Page<Vault>>>;
  getVault(id: VaultId): Promise<ApiResult<Vault>>;
  /** The first objects of each kind in a vault. */
  listObjects(id: VaultId): Promise<ApiResult<readonly ObjectSummary[]>>;

  listWikiPages(vault: VaultId, cursor: string | null): Promise<ApiResult<Page<WikiPageSummary>>>;
  /** A past revision when `revision` is set. Claims show what stands today either way. */
  getWikiPage(vault: VaultId, id: string, revision: number | null): Promise<ApiResult<WikiPage>>;
  resolveWikiSlug(vault: VaultId, slug: string): Promise<ApiResult<WikiRedirect>>;
  listWikiBacklinks(vault: VaultId, id: string, cursor: string | null): Promise<ApiResult<Page<WikiBacklink>>>;
  listWikiLinks(vault: VaultId, id: string, cursor: string | null): Promise<ApiResult<Page<WikiOutgoingLink>>>;
  listWikiUnresolved(vault: VaultId, cursor: string | null): Promise<ApiResult<Page<WikiUnresolvedLink>>>;
  listWikiRevisions(vault: VaultId, id: string, cursor: string | null): Promise<ApiResult<Page<WikiRevision>>>;
  getWikiDiff(vault: VaultId, id: string, from: number, to: number): Promise<ApiResult<WikiDiff>>;
  /** Who a revision's change came from. */
  getWikiAttribution(vault: VaultId, changesetId: string): Promise<ApiResult<WikiAttribution>>;
  exportWikiPage(vault: VaultId, id: string): Promise<ApiResult<string>>;
  createWikiPage(vault: VaultId, input: NewWikiPage): Promise<ApiResult<{ readonly pageId: string; readonly write: WikiWrite }>>;
  editWikiPage(vault: VaultId, id: string, input: WikiEdit): Promise<ApiResult<WikiWrite>>;
  renameWikiPage(vault: VaultId, id: string, input: WikiRename): Promise<ApiResult<WikiWrite>>;
  revertWikiPage(vault: VaultId, id: string, input: WikiRevert): Promise<ApiResult<WikiWrite>>;
  deleteWikiPage(vault: VaultId, id: string, input: WikiWriteBase): Promise<ApiResult<WikiWrite>>;
  importWikiPage(vault: VaultId, input: WikiImport): Promise<ApiResult<{ readonly pageId: string; readonly created: boolean; readonly write: WikiWrite }>>;
  /** The claims a keyword search finds, at most `limit`. */
  searchClaimIds(vault: VaultId, query: string, limit: number): Promise<ApiResult<readonly string[]>>;
  /** One claim as it stands in its own right, and what replaced it if anything did. */
  readClaim(vault: VaultId, id: string): Promise<ApiResult<{ readonly claim: WikiClaim; readonly supersededBy: string | null }>>;

  /** A failure includes no answer within `timeoutMs`. */
  health(timeoutMs: number): Promise<ApiResult<null>>;
}

export const isUnauthenticated = (failure: ApiFailure): boolean => failure.kind === "http" && failure.status === 401;
export const isForbidden = (failure: ApiFailure): boolean => failure.kind === "http" && failure.status === 403;
export const isNotFound = (failure: ApiFailure): boolean => failure.kind === "http" && failure.status === 404;

/** A request id, if the server gave one, in the one form every screen shows it. */
export const requestIdText = (failure: ApiFailure): string | null => (failure.kind === "http" ? `request id: ${failure.requestId}` : null);
