import type { ChatSettings, ClearedCaches, CreatedToken, ErasureImpact, ErasureRecord, Grant, ImportRun, McpInfo, OperationJob, Page, PrincipalRow, SystemStatus, TokenRow, Usage, Vault, VaultId, WikiWrite } from "@memex/types";
import { failed, ok } from "./api";
import type { ApiResult } from "./api";
import type { OperationRequest } from "./admin";
import type { UploadFile } from "./upload-queue";

export interface NewPrincipal {
  readonly name: string;
  readonly kind: "human" | "agent";
  readonly serverAdmin: boolean;
}

export interface NewToken {
  readonly name: string;
  /** Null makes it the caller's own. */
  readonly principalId: string | null;
  readonly grants: readonly Grant[];
  readonly expiresAtMs: number | null;
}

export interface ImportOptions {
  /** Restore under a new id rather than the archive's. */
  readonly newId: boolean;
  /** Overwrite the vault with the archive's id if there is one. */
  readonly replace: boolean;
  readonly name: string | null;
}

export interface ChatSettingsChange {
  readonly enabled: boolean;
  readonly threadTokenBudget: number | null;
  readonly dayTokenBudget: number | null;
}

/** The admin API: vaults, principals, tokens, erasure, operations, imports and the server's own state (specs 030, 039 and 049). */
export interface AdminApi {
  createVault(name: string): Promise<ApiResult<Vault>>;
  renameVault(id: VaultId, name: string): Promise<ApiResult<Vault>>;
  deleteVault(id: VaultId): Promise<ApiResult<null>>;
  listPrincipals(cursor: string | null): Promise<ApiResult<Page<PrincipalRow>>>;
  createPrincipal(input: NewPrincipal): Promise<ApiResult<PrincipalRow>>;
  disablePrincipal(id: string): Promise<ApiResult<null>>;
  /** `all` lists everyone's, which needs a server admin. Otherwise the caller's own. */
  listTokens(all: boolean, cursor: string | null): Promise<ApiResult<Page<TokenRow>>>;
  createToken(input: NewToken): Promise<ApiResult<CreatedToken>>;
  revokeToken(id: string): Promise<ApiResult<null>>;
  getErasurePreview(vault: VaultId, source: string): Promise<ApiResult<ErasureImpact>>;
  proposeErasure(vault: VaultId, source: string, rationale: string): Promise<ApiResult<WikiWrite>>;
  listErasures(vault: VaultId, cursor: string | null): Promise<ApiResult<Page<ErasureRecord>>>;
  listOperations(vault: VaultId, cursor: string | null): Promise<ApiResult<Page<OperationJob>>>;
  startOperation(vault: VaultId, request: OperationRequest): Promise<ApiResult<OperationJob>>;
  getOperation(vault: VaultId, id: string): Promise<ApiResult<OperationJob>>;
  startImport(file: UploadFile, options: ImportOptions): Promise<ApiResult<ImportRun>>;
  getImport(id: string): Promise<ApiResult<ImportRun>>;
  getSystemSettings(): Promise<ApiResult<Readonly<Record<string, unknown>>>>;
  getSystemStatus(): Promise<ApiResult<SystemStatus>>;
  /** UTC days as `YYYY-MM-DD`, or null for the server's default. */
  getUsage(from: string | null, to: string | null): Promise<ApiResult<Usage>>;
  getMcpInfo(): Promise<ApiResult<McpInfo>>;
  clearCaches(): Promise<ApiResult<ClearedCaches>>;
  putChatSettings(vault: VaultId, change: ChatSettingsChange): Promise<ApiResult<ChatSettings>>;
}

export interface AdminCases {
  createVault(name: string): Promise<ApiResult<Vault>>;
  renameVault(id: VaultId, name: string): Promise<ApiResult<Vault>>;
  deleteVault(id: VaultId): Promise<ApiResult<null>>;
  /** Every principal, page by page, up to a bound. */
  listPrincipals(): Promise<ApiResult<readonly PrincipalRow[]>>;
  createPrincipal(input: NewPrincipal): Promise<ApiResult<PrincipalRow>>;
  disablePrincipal(id: string): Promise<ApiResult<null>>;
  listTokens(all: boolean): Promise<ApiResult<readonly TokenRow[]>>;
  createToken(input: NewToken): Promise<ApiResult<CreatedToken>>;
  revokeToken(id: string): Promise<ApiResult<null>>;
  getErasurePreview(vault: VaultId, source: string): Promise<ApiResult<ErasureImpact>>;
  proposeErasure(vault: VaultId, source: string, rationale: string): Promise<ApiResult<WikiWrite>>;
  listErasures(vault: VaultId): Promise<ApiResult<readonly ErasureRecord[]>>;
  listOperations(vault: VaultId): Promise<ApiResult<readonly OperationJob[]>>;
  startOperation(vault: VaultId, request: OperationRequest): Promise<ApiResult<OperationJob>>;
  getOperation(vault: VaultId, id: string): Promise<ApiResult<OperationJob>>;
  startImport(file: UploadFile, options: ImportOptions): Promise<ApiResult<ImportRun>>;
  getImport(id: string): Promise<ApiResult<ImportRun>>;
  getSystemSettings(): Promise<ApiResult<Readonly<Record<string, unknown>>>>;
  getSystemStatus(): Promise<ApiResult<SystemStatus>>;
  getUsage(from: string | null, to: string | null): Promise<ApiResult<Usage>>;
  getMcpInfo(): Promise<ApiResult<McpInfo>>;
  clearCaches(): Promise<ApiResult<ClearedCaches>>;
  putChatSettings(vault: VaultId, change: ChatSettingsChange): Promise<ApiResult<ChatSettings>>;
}

interface AdminDeps {
  readonly edges: { readonly api: AdminApi };
  readonly watched: <T>(call: Promise<ApiResult<T>>) => Promise<ApiResult<T>>;
}

/** 20 pages of 200 is 4,000 rows, which no list here should reach. A server that loops its cursors stops there. */
const MAX_PAGES = 20;

const allPages = async <T>(read: (cursor: string | null) => Promise<ApiResult<Page<T>>>): Promise<ApiResult<readonly T[]>> => {
  const all: T[] = [];
  let cursor: string | null = null;
  for (let n = 0; n < MAX_PAGES; n += 1) {
    const result: ApiResult<Page<T>> = await read(cursor);
    if (!result.ok) return failed(result.failure);
    all.push(...result.value.items);
    cursor = result.value.nextCursor;
    if (cursor === null) break;
  }
  return ok(all);
};

export const createAdminCases = ({ edges: { api }, watched }: AdminDeps): AdminCases => ({
  createVault: (name) => watched(api.createVault(name.trim())),
  renameVault: (id, name) => watched(api.renameVault(id, name.trim())),
  deleteVault: (id) => watched(api.deleteVault(id)),
  listPrincipals: () => allPages((cursor) => watched(api.listPrincipals(cursor))),
  createPrincipal: (input) => watched(api.createPrincipal({ ...input, name: input.name.trim() })),
  disablePrincipal: (id) => watched(api.disablePrincipal(id)),
  listTokens: (all) => allPages((cursor) => watched(api.listTokens(all, cursor))),
  createToken: (input) => watched(api.createToken({ ...input, name: input.name.trim() })),
  revokeToken: (id) => watched(api.revokeToken(id)),
  getErasurePreview: (vault, source) => watched(api.getErasurePreview(vault, source)),
  proposeErasure: (vault, source, rationale) => watched(api.proposeErasure(vault, source, rationale)),
  listErasures: (vault) => allPages((cursor) => watched(api.listErasures(vault, cursor))),
  // The server lists them in id order, so the newest come first only once they're sorted.
  listOperations: async (vault) => {
    const result = await allPages((cursor) => watched(api.listOperations(vault, cursor)));
    return result.ok ? ok([...result.value].sort((a, b) => b.createdAtMs - a.createdAtMs || b.id.localeCompare(a.id))) : result;
  },
  startOperation: (vault, request) => watched(api.startOperation(vault, request)),
  getOperation: (vault, id) => watched(api.getOperation(vault, id)),
  startImport: (file, options) => watched(api.startImport(file, options)),
  getImport: (id) => watched(api.getImport(id)),
  getSystemSettings: () => watched(api.getSystemSettings()),
  getSystemStatus: () => watched(api.getSystemStatus()),
  getUsage: (from, to) => watched(api.getUsage(from, to)),
  getMcpInfo: () => watched(api.getMcpInfo()),
  clearCaches: () => watched(api.clearCaches()),
  putChatSettings: (vault, change) => watched(api.putChatSettings(vault, change)),
});
