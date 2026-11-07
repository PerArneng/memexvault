import type { ChangeOrigin, CitingClaim, ClaimLineage, Derivation, DocumentSource, FlaggedPage, MediaFacts, Page, PageList, PageWord, Rendition, Segment, Transcript, VaultId } from "@memex/types";
import { failed, ok } from "./api";
import type { Api, ApiFailure, ApiResult, TranscriptRequest } from "./api";
import type { IdSource } from "./wiki-cases";

export interface SourceList {
  readonly sources: readonly DocumentSource[];
  /** There are more than can be listed in the browser, so filters only saw these. */
  readonly truncated: boolean;
}

export type SourceTrustOutcome =
  | { readonly kind: "saved"; readonly changesetId: string | null }
  /** It waits for a person to approve it in Review, so the tier hasn't changed yet. */
  | { readonly kind: "pending"; readonly changesetId: string | null }
  | { readonly kind: "refused"; readonly reasons: readonly string[] }
  /** Someone changed the source first, and nothing was done. */
  | { readonly kind: "stale" }
  | { readonly kind: "failed"; readonly failure: ApiFailure };

export interface DocumentCases {
  listSources(vault: VaultId): Promise<ApiResult<SourceList>>;
  getSource(vault: VaultId, id: string): Promise<ApiResult<DocumentSource>>;
  listSourceVersions(vault: VaultId, id: string): Promise<ApiResult<readonly DocumentSource[]>>;
  listRenditions(vault: VaultId, source: string): Promise<ApiResult<readonly Rendition[]>>;
  listPageWords(vault: VaultId, rendition: string): Promise<ApiResult<readonly PageWord[]>>;
  listSegments(vault: VaultId, rendition: string, cursor: string | null): Promise<ApiResult<Page<Segment>>>;
  getSegment(vault: VaultId, id: string): Promise<ApiResult<Segment>>;
  listFlaggedPages(vault: VaultId): Promise<ApiResult<readonly FlaggedPage[]>>;
  listPages(vault: VaultId, source: string): Promise<ApiResult<PageList>>;
  checkPageImage(vault: VaultId, source: string, page: number, dpi: number): Promise<ApiResult<null>>;
  listSourceClaims(vault: VaultId, source: string, cursor: string | null): Promise<ApiResult<Page<CitingClaim>>>;
  getLineage(vault: VaultId, claim: string): Promise<ApiResult<ClaimLineage>>;
  getChangeOrigin(vault: VaultId, changesetId: string): Promise<ApiResult<ChangeOrigin>>;
  /** The derivation that read this segment, if any did. */
  findDerivation(vault: VaultId, rendition: string, segment: string): Promise<ApiResult<Derivation | null>>;
  getMedia(vault: VaultId, source: string): Promise<ApiResult<MediaFacts>>;
  getTranscript(vault: VaultId, source: string, request: TranscriptRequest): Promise<ApiResult<Transcript>>;
  reprocessSource(vault: VaultId, source: string): Promise<ApiResult<null>>;
  setSourceTrust(vault: VaultId, source: string, input: { readonly baseVersion: number; readonly tier: string; readonly rationale: string }): Promise<SourceTrustOutcome>;
  /** Same-origin addresses, so the session cookie and the CSP apply to images and media as they do to the API. */
  pageImageUrl(vault: VaultId, source: string, page: number, dpi: number): string;
  thumbnailUrl(vault: VaultId, source: string, page: number): string;
  streamUrl(vault: VaultId, source: string): string;
  fileUrl(vault: VaultId, source: string): string;
}

/** The cursors of a list can't run for ever: 50 pages of 200 is 10,000 rows, which is as many as a browser list should hold. */
const MAX_LIST_PAGES = 50;

interface DocumentDeps {
  readonly edges: { readonly api: Api; readonly ids: IdSource; readonly settings: { readonly apiBaseUrl: string } };
  readonly watched: <T>(call: Promise<ApiResult<T>>) => Promise<ApiResult<T>>;
}

const isStale = (failure: ApiFailure): boolean => failure.kind === "http" && failure.code === "stale_version";

export const createDocumentCases = ({ edges, watched }: DocumentDeps): DocumentCases => {
  const { api, ids, settings } = edges;
  const sourceRoot = (vault: VaultId, source: string): string => `${settings.apiBaseUrl}/vaults/${encodeURIComponent(vault)}/sources/${encodeURIComponent(source)}`;

  /** A write with no answer may still have landed, so a retry reuses its key and the server replays the first result. */
  const trustKeys = new Map<string, string>();

  const allPages = async <T>(read: (cursor: string | null) => Promise<ApiResult<Page<T>>>): Promise<ApiResult<{ readonly items: readonly T[]; readonly truncated: boolean }>> => {
    const items: T[] = [];
    let cursor: string | null = null;
    for (let page = 0; page < MAX_LIST_PAGES; page += 1) {
      const result: ApiResult<Page<T>> = await watched(read(cursor));
      if (!result.ok) return failed(result.failure);
      items.push(...result.value.items);
      cursor = result.value.nextCursor;
      if (cursor === null) return ok({ items, truncated: false });
    }
    return ok({ items, truncated: true });
  };

  return {
    listSources: async (vault) => {
      const result = await allPages((cursor) => api.listSources(vault, cursor));
      return result.ok ? ok({ sources: result.value.items, truncated: result.value.truncated }) : failed(result.failure);
    },
    getSource: (vault, id) => watched(api.getSource(vault, id)),
    listSourceVersions: (vault, id) => watched(api.listSourceVersions(vault, id)),
    listRenditions: (vault, source) => watched(api.listRenditions(vault, source)),
    listPageWords: (vault, rendition) => watched(api.listPageWords(vault, rendition)),
    listSegments: (vault, rendition, cursor) => watched(api.listSegments(vault, rendition, cursor)),
    getSegment: (vault, id) => watched(api.getSegment(vault, id)),
    listFlaggedPages: async (vault) => {
      const result = await allPages((cursor) => api.listFlaggedPages(vault, cursor));
      return result.ok ? ok(result.value.items) : failed(result.failure);
    },
    listPages: (vault, source) => watched(api.listPages(vault, source)),
    checkPageImage: (vault, source, page, dpi) => watched(api.checkPageImage(vault, source, page, dpi)),
    listSourceClaims: (vault, source, cursor) => watched(api.listSourceClaims(vault, source, cursor)),
    getLineage: (vault, claim) => watched(api.getLineage(vault, claim)),
    getChangeOrigin: (vault, changesetId) => watched(api.getChangeOrigin(vault, changesetId)),
    findDerivation: async (vault, rendition, segment) => {
      const result = await allPages((cursor) => api.listDerivations(vault, rendition, cursor));
      return result.ok ? ok(result.value.items.find((d) => d.segmentIds.includes(segment)) ?? null) : failed(result.failure);
    },
    getMedia: (vault, source) => watched(api.getMedia(vault, source)),
    getTranscript: (vault, source, request) => watched(api.getTranscript(vault, source, request)),
    reprocessSource: (vault, source) => watched(api.reprocessSource(vault, source)),
    setSourceTrust: async (vault, source, { baseVersion, tier, rationale }) => {
      const what = `${vault}/${source}/${String(baseVersion)}/${tier}`;
      const key = trustKeys.get(what) ?? ids.next();
      trustKeys.set(what, key);
      const result = await watched(api.setSourceTrust(vault, source, { baseVersion, tier, rationale, idempotencyKey: key }));
      if (result.ok || result.failure.kind !== "unreachable") trustKeys.delete(what);
      if (!result.ok) return isStale(result.failure) ? { kind: "stale" } : { kind: "failed", failure: result.failure };
      const { state, changesetId, refusals } = result.value;
      if (state === "committed") return { kind: "saved", changesetId };
      if (state === "pending") return { kind: "pending", changesetId };
      return { kind: "refused", reasons: refusals };
    },
    pageImageUrl: (vault, source, page, dpi) => `${sourceRoot(vault, source)}/pages/${String(page)}/image?dpi=${String(dpi)}`,
    thumbnailUrl: (vault, source, page) => `${sourceRoot(vault, source)}/pages/${String(page)}/thumbnail`,
    streamUrl: (vault, source) => `${sourceRoot(vault, source)}/stream`,
    fileUrl: (vault, source) => `${sourceRoot(vault, source)}/file`,
  };
};
