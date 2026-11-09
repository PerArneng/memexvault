import type { EvidencePackage, RenditionReading, VaultId } from "@memex/types";
import type { Api, ApiResult, CancelSignal } from "./api";
import type { SearchRequest } from "./search";

export interface SearchCases {
  search(vault: VaultId, request: SearchRequest, cancel: CancelSignal | null): Promise<ApiResult<EvidencePackage>>;
  getRenditionReading(vault: VaultId, rendition: string): Promise<ApiResult<RenditionReading>>;
}

interface SearchDeps {
  readonly edges: { readonly api: Api };
  readonly watched: <T>(call: Promise<ApiResult<T>>) => Promise<ApiResult<T>>;
}

export const createSearchCases = ({ edges: { api }, watched }: SearchDeps): SearchCases => ({
  search: async (vault, request, cancel) => {
    const answer = await api.search(vault, request, cancel);
    // A search somebody typed past looks like no answer from the server, which it isn't, so it mustn't count against the connection.
    return cancel?.aborted === true ? answer : watched(Promise.resolve(answer));
  },
  getRenditionReading: (vault, rendition) => watched(api.getRenditionReading(vault, rendition)),
});
