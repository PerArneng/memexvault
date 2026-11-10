import type {
  ApiFailure,
  ChangeOrigin,
  CitingClaim,
  ClaimLineage,
  Derivation,
  DocumentSource,
  EntityDetail,
  EntityRedirect,
  EntityResolution,
  EntitySummary,
  EntityType,
  EntityTypePreview,
  EvidencePackage,
  FlaggedPage,
  Identity,
  MediaFacts,
  Neighbor,
  ObjectSummary,
  Page,
  PageList,
  PageWord,
  Rendition,
  SameAsSuggestion,
  RenditionReading,
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
import type { SearchRequest } from "./search";

import type { IngestApi } from "./ingest-cases";

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

export interface NewEntityType {
  readonly name: string;
  readonly baseVersion: number | null;
  readonly schema: unknown;
  readonly rationale: string;
  readonly idempotencyKey: string;
}

export interface EntityEvidenceInput {
  readonly segmentId: string;
  readonly quote: string;
}

export interface EntityFieldInput {
  readonly field: string;
  readonly value: unknown;
  readonly evidence: readonly EntityEvidenceInput[];
  /** The claim this replaces. Without it, the value goes beside the others. */
  readonly replaces: { readonly assertionId: string; readonly version: number } | null;
}

export interface NewEntityInput {
  readonly type: string;
  readonly aliases: readonly string[];
  readonly fields: readonly EntityFieldInput[];
  readonly rationale: string;
  readonly idempotencyKey: string;
}

export interface EntityEditInput {
  readonly baseVersion: number;
  /** Null keeps them. */
  readonly aliases: readonly string[] | null;
  readonly fields: readonly EntityFieldInput[];
  readonly rationale: string;
  readonly idempotencyKey: string;
}

export interface EntityMergeInput {
  readonly into: string;
  readonly rationale: string;
  readonly idempotencyKey: string;
}

export interface EntitySplitInput {
  readonly baseVersion: number;
  readonly moved: readonly string[];
  readonly aliases: readonly string[];
  readonly fields: readonly EntityFieldInput[];
  readonly rationale: string;
  readonly idempotencyKey: string;
}

export interface EntityListRequest {
  readonly type: string;
  /** `<field>:<op>:<value>`. */
  readonly filters: readonly string[];
  readonly cursor: string | null;
}

/** What this layer needs of an `AbortSignal`, as a pure package can't name that. */
export interface CancelSignal {
  readonly aborted: boolean;
}

export interface Api extends IngestApi {
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
  listEntityTypes(vault: VaultId, cursor: string | null): Promise<ApiResult<Page<EntityType>>>;
  listEntityTypeVersions(vault: VaultId, name: string): Promise<ApiResult<readonly EntityType[]>>;
  previewEntityType(vault: VaultId, name: string, schema: unknown): Promise<ApiResult<EntityTypePreview>>;
  submitEntityType(vault: VaultId, input: NewEntityType): Promise<ApiResult<WikiWrite>>;
  listEntities(vault: VaultId, request: EntityListRequest): Promise<ApiResult<Page<EntitySummary>>>;
  getEntity(vault: VaultId, id: string): Promise<ApiResult<EntityDetail>>;
  getEntityRedirect(vault: VaultId, id: string): Promise<ApiResult<EntityRedirect>>;
  createEntity(vault: VaultId, input: NewEntityInput): Promise<ApiResult<{ readonly entityId: string; readonly write: WikiWrite }>>;
  editEntity(vault: VaultId, id: string, input: EntityEditInput): Promise<ApiResult<WikiWrite>>;
  mergeEntity(vault: VaultId, id: string, input: EntityMergeInput): Promise<ApiResult<WikiWrite>>;
  splitEntity(vault: VaultId, id: string, input: EntitySplitInput): Promise<ApiResult<{ readonly entityId: string; readonly write: WikiWrite }>>;
  resolveEntities(vault: VaultId, mention: string, type: string | null): Promise<ApiResult<EntityResolution>>;
  listSameAs(vault: VaultId, type: string, cursor: string | null): Promise<ApiResult<Page<SameAsSuggestion>>>;
  listNeighbors(vault: VaultId, id: string): Promise<ApiResult<Page<Neighbor>>>;
  /** A claim's version as it stands, for a value that replaces it. Null if the server doesn't say. */
  readClaimVersion(vault: VaultId, id: string): Promise<ApiResult<number | null>>;
  /** One rendition's text with its word table, which is large on a long document. */
  getRenditionReading(vault: VaultId, rendition: string): Promise<ApiResult<RenditionReading>>;

  /** The search API. `cancel` abandons a request that's still running. */
  search(vault: VaultId, request: SearchRequest, cancel: CancelSignal | null): Promise<ApiResult<EvidencePackage>>;

  /** A failure includes no answer within `timeoutMs`. */
  health(timeoutMs: number): Promise<ApiResult<null>>;
}

export const isUnauthenticated = (failure: ApiFailure): boolean => failure.kind === "http" && failure.status === 401;
export const isForbidden = (failure: ApiFailure): boolean => failure.kind === "http" && failure.status === 403;
export const isNotFound = (failure: ApiFailure): boolean => failure.kind === "http" && failure.status === 404;

/** A request id, if the server gave one, in the one form every screen shows it. */
export const requestIdText = (failure: ApiFailure): string | null => (failure.kind === "http" ? `request id: ${failure.requestId}` : null);
