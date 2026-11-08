import type {
  ApiFailure,
  ChangeOrigin,
  CitingClaim,
  ClaimLineage,
  Derivation,
  DocumentSource,
  FlaggedPage,
  Identity,
  MediaFacts,
  ObjectSummary,
  Page,
  PageList,
  PageWord,
  Rendition,
  Segment,
  Transcript,
  Vault,
  VaultId,
  AutoCommitEntry,
  BatchOutcome,
  ChangeSet,
  Contradiction,
  InversePreview,
  QueueCounts,
  ReviewDecision,
  ReviewDetail,
  ReviewFilter,
  ReviewItem,
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

/** Which stretch of a transcript to read. */
export interface TranscriptRequest {
  readonly fromMs: number | null;
  readonly cursor: string | null;
  readonly words: boolean;
}

export interface TrustChange {
  readonly baseVersion: number;
  readonly tier: string;
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

  listSources(vault: VaultId, cursor: string | null): Promise<ApiResult<Page<DocumentSource>>>;
  getSource(vault: VaultId, id: string): Promise<ApiResult<DocumentSource>>;
  listSourceVersions(vault: VaultId, id: string): Promise<ApiResult<readonly DocumentSource[]>>;
  listRenditions(vault: VaultId, source: string): Promise<ApiResult<readonly Rendition[]>>;
  /** The whole word table, so it's large on a long document and read only when someone asks to select text. */
  listPageWords(vault: VaultId, rendition: string): Promise<ApiResult<readonly PageWord[]>>;
  listSegments(vault: VaultId, rendition: string, cursor: string | null): Promise<ApiResult<Page<Segment>>>;
  getSegment(vault: VaultId, id: string): Promise<ApiResult<Segment>>;
  listFlaggedPages(vault: VaultId, cursor: string | null): Promise<ApiResult<Page<FlaggedPage>>>;
  listPages(vault: VaultId, source: string): Promise<ApiResult<PageList>>;
  /** Asks for a page image only to learn why the browser couldn't show it, since an image's own error says nothing. */
  checkPageImage(vault: VaultId, source: string, page: number, dpi: number): Promise<ApiResult<null>>;
  listSourceClaims(vault: VaultId, source: string, cursor: string | null): Promise<ApiResult<Page<CitingClaim>>>;
  getLineage(vault: VaultId, claim: string): Promise<ApiResult<ClaimLineage>>;
  getChangeOrigin(vault: VaultId, changesetId: string): Promise<ApiResult<ChangeOrigin>>;
  listDerivations(vault: VaultId, rendition: string, cursor: string | null): Promise<ApiResult<Page<Derivation>>>;
  getMedia(vault: VaultId, source: string): Promise<ApiResult<MediaFacts>>;
  getTranscript(vault: VaultId, source: string, request: TranscriptRequest): Promise<ApiResult<Transcript>>;
  reprocessSource(vault: VaultId, source: string): Promise<ApiResult<null>>;
  setSourceTrust(vault: VaultId, source: string, change: TrustChange): Promise<ApiResult<WikiWrite>>;

  listReviewQueue(vault: VaultId, filter: ReviewFilter, cursor: string | null): Promise<ApiResult<Page<ReviewItem>>>;
  getQueueCounts(vault: VaultId): Promise<ApiResult<QueueCounts>>;
  getReviewItem(vault: VaultId, id: string): Promise<ApiResult<ReviewDetail>>;
  decide(vault: VaultId, decision: ReviewDecision): Promise<ApiResult<ChangeSet>>;
  decideBatch(vault: VaultId, decisions: readonly ReviewDecision[]): Promise<ApiResult<readonly BatchOutcome[]>>;
  listChangesets(vault: VaultId, state: string, cursor: string | null): Promise<ApiResult<Page<ChangeSet>>>;
  getChangeset(vault: VaultId, id: string): Promise<ApiResult<ChangeSet>>;
  getInverse(vault: VaultId, id: string): Promise<ApiResult<InversePreview>>;
  revertChangeset(vault: VaultId, id: string, reason: string): Promise<ApiResult<ChangeSet>>;
  listContradictions(vault: VaultId, cursor: string | null): Promise<ApiResult<Page<Contradiction>>>;
  listAutoCommit(vault: VaultId, cursor: string | null): Promise<ApiResult<{ readonly threshold: number; readonly entries: readonly AutoCommitEntry[]; readonly nextCursor: string | null }>>;
  disableAutoCommit(vault: VaultId, combination: string, reason: string): Promise<ApiResult<AutoCommitEntry>>;

  /** A failure includes no answer within `timeoutMs`. */
  health(timeoutMs: number): Promise<ApiResult<null>>;
}

export const isUnauthenticated = (failure: ApiFailure): boolean => failure.kind === "http" && failure.status === 401;
export const isForbidden = (failure: ApiFailure): boolean => failure.kind === "http" && failure.status === 403;
export const isNotFound = (failure: ApiFailure): boolean => failure.kind === "http" && failure.status === 404;

/** A request id, if the server gave one, in the one form every screen shows it. */
export const requestIdText = (failure: ApiFailure): string | null => (failure.kind === "http" ? `request id: ${failure.requestId}` : null);
