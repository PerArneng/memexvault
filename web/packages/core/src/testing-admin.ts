import { epochMs, vaultId } from "@memex/types";
import type { ChatSettings, ErasureImpact, ErasureRecord, ImportRun, McpInfo, OperationJob, Page, PrincipalRow, SystemStatus, TokenRow, Usage, Vault, WikiWrite } from "@memex/types";
import { failed, ok } from "./api";
import type { ApiFailure, ApiResult } from "./api";
import type { AdminApi } from "./admin-cases";

export interface AdminSeed {
  readonly principals?: readonly PrincipalRow[];
  readonly tokens?: readonly TokenRow[];
  /** Each source's preview. A source left out is not found. */
  readonly previews?: Readonly<Record<string, ErasureImpact>>;
  readonly erasures?: readonly ErasureRecord[];
  readonly operations?: readonly OperationJob[];
  readonly status?: SystemStatus;
  readonly settings?: Readonly<Record<string, unknown>>;
  readonly usage?: Usage;
  readonly mcp?: McpInfo;
}

/** The admin routes on a fake server, with what they change kept in memory so a test can read it back. */
export interface FakeAdmin extends AdminApi {
  readonly vaultsMade: Vault[];
  readonly deletedVaults: string[];
  readonly principalRows: PrincipalRow[];
  readonly tokenRows: TokenRow[];
  readonly operationRows: OperationJob[];
  /** Every token value the fake has handed out. */
  readonly secrets: string[];
  chatSettings: ChatSettings | null;
}

const NOT_FOUND: ApiFailure = { kind: "http", status: 404, code: "not_found", message: "not_found", requestId: "req-1" };
const conflict = (message: string): ApiFailure => ({ kind: "http", status: 409, code: "conflict", message, requestId: "req-1" });

const pageOf = <T>(all: readonly T[], cursor: string | null, size = 2): Page<T> => {
  const start = cursor === null ? 0 : Number(cursor);
  const end = start + size;
  return { items: all.slice(start, end), nextCursor: end < all.length ? String(end) : null };
};

export const EMPTY_STATUS: SystemStatus = {
  version: "0.0.0",
  postgres: { configured: true, reachable: true, latencyMs: 3, error: null },
  llm: { configured: false, reachable: false, latencyMs: null, error: null },
  abandonedHandlers: 0,
  memoryRssBytes: 100_000_000,
  models: [],
  caches: [],
  vaults: [],
  vaultsError: null,
  callerFromMemory: false,
  backends: {},
  thresholds: {},
};

export const fakeAdminApi = (seed: AdminSeed, answer: <T>(name: string, value: () => ApiResult<T>) => Promise<ApiResult<T>>): FakeAdmin => {
  let count = 0;
  const next = (prefix: string) => {
    count += 1;
    return `${prefix}-${String(count)}`;
  };
  const fake: FakeAdmin = {
    vaultsMade: [],
    deletedVaults: [],
    principalRows: [...(seed.principals ?? [])],
    tokenRows: [...(seed.tokens ?? [])],
    operationRows: [...(seed.operations ?? [])],
    secrets: [],
    chatSettings: null,
    createVault: (name) =>
      answer(`createVault:${name}`, () => {
        if (fake.vaultsMade.some((v) => v.name === name)) return failed(conflict("a vault has that name"));
        const vault: Vault = { id: vaultId(next("vault")), name, createdAt: epochMs(0) };
        fake.vaultsMade.push(vault);
        return ok(vault);
      }),
    renameVault: (id, name) => answer(`renameVault:${id}:${name}`, () => ok({ id, name, createdAt: epochMs(0) })),
    deleteVault: (id) =>
      answer(`deleteVault:${id}`, () => {
        fake.deletedVaults.push(id);
        return ok(null);
      }),
    listPrincipals: (cursor) => answer("listPrincipals", () => ok(pageOf(fake.principalRows, cursor))),
    createPrincipal: (input) =>
      answer(`createPrincipal:${input.name}`, () => {
        const row: PrincipalRow = { id: next("principal"), name: input.name, kind: input.kind, serverAdmin: input.serverAdmin, disabled: false, createdBy: "p1", createdAtMs: 0 };
        fake.principalRows.push(row);
        return ok(row);
      }),
    disablePrincipal: (id) =>
      answer(`disablePrincipal:${id}`, () => {
        const i = fake.principalRows.findIndex((p) => p.id === id);
        const row = fake.principalRows[i];
        if (row === undefined) return failed(NOT_FOUND);
        fake.principalRows[i] = { ...row, disabled: true };
        return ok(null);
      }),
    listTokens: (all, cursor) => answer(`listTokens:${String(all)}`, () => ok(pageOf(all ? fake.tokenRows : fake.tokenRows.filter((t) => t.principalId === "p1"), cursor))),
    createToken: (input) =>
      answer(`createToken:${input.name}`, () => {
        const id = next("token");
        const token = `secret-${id}`;
        const details: TokenRow = { id, principalId: input.principalId ?? "p1", name: input.name, grants: input.grants, createdBy: "p1", createdAtMs: 0, expiresAtMs: input.expiresAtMs, lastUsedAtMs: null, revokedAtMs: null };
        fake.tokenRows.push(details);
        fake.secrets.push(token);
        return ok({ token, details });
      }),
    revokeToken: (id) =>
      answer(`revokeToken:${id}`, () => {
        const i = fake.tokenRows.findIndex((t) => t.id === id);
        const row = fake.tokenRows[i];
        if (row === undefined) return failed(NOT_FOUND);
        fake.tokenRows[i] = { ...row, revokedAtMs: 1 };
        return ok(null);
      }),
    getErasurePreview: (_vault, source) =>
      answer(`getErasurePreview:${source}`, () => {
        const preview = seed.previews?.[source];
        return preview === undefined ? failed(NOT_FOUND) : ok(preview);
      }),
    proposeErasure: (_vault, source, rationale) =>
      answer(`proposeErasure:${source}:${rationale}`, (): ApiResult<WikiWrite> => (seed.previews?.[source] === undefined ? failed(NOT_FOUND) : ok({ changesetId: `cs-erase-${source}`, state: "pending", replayed: false, refusals: [] }))),
    listErasures: (_vault, cursor) => answer("listErasures", () => ok(pageOf(seed.erasures ?? [], cursor))),
    listOperations: (_vault, cursor) => answer("listOperations", () => ok(pageOf(fake.operationRows, cursor))),
    startOperation: (_vault, request) =>
      answer(`startOperation:${JSON.stringify(request)}`, () => {
        if (fake.operationRows.some((o) => o.op === request.op && (o.state === "queued" || o.state === "running"))) return failed(conflict(`a ${request.op} is already queued`));
        const job: OperationJob = { id: next("job"), op: request.op, input: { op: request.op }, state: "queued", progress: null, report: null, createdAtMs: count, finishedAtMs: null, error: null };
        fake.operationRows.push(job);
        return ok(job);
      }),
    getOperation: (_vault, id) =>
      answer(`getOperation:${id}`, () => {
        const job = fake.operationRows.find((o) => o.id === id);
        return job === undefined ? failed(NOT_FOUND) : ok(job);
      }),
    startImport: (file, options) =>
      answer(`startImport:${file.name}:${JSON.stringify(options)}`, (): ApiResult<ImportRun> => ok({ id: next("import"), state: "running", progress: { step: 1, of: 3, label: "reading" }, report: null, error: null, startedAtMs: 0, finishedAtMs: null })),
    getImport: (id) => answer(`getImport:${id}`, (): ApiResult<ImportRun> => ok({ id, state: "done", progress: { step: 3, of: 3, label: "done" }, report: { vault: "vault-restored" }, error: null, startedAtMs: 0, finishedAtMs: 1 })),
    getSystemSettings: () => answer("getSystemSettings", () => ok(seed.settings ?? {})),
    getSystemStatus: () => answer("getSystemStatus", () => ok(seed.status ?? EMPTY_STATUS)),
    getUsage: (from, to) => answer(`getUsage:${from ?? ""}:${to ?? ""}`, () => ok(seed.usage ?? { rows: [], pricedCostUsd: 0, unpricedTokens: 0 })),
    getMcpInfo: () => answer("getMcpInfo", () => (seed.mcp === undefined ? failed(NOT_FOUND) : ok(seed.mcp))),
    clearCaches: () => answer("clearCaches", () => ok({ vaults: 1, caches: [] })),
    putChatSettings: (_vault, change) =>
      answer(`putChatSettings:${JSON.stringify(change)}`, () => {
        const settings: ChatSettings = { enabled: change.enabled, threadTokenBudget: change.threadTokenBudget, dayTokenBudget: change.dayTokenBudget, dayTokensUsed: 0, dayTokensLimit: change.dayTokenBudget ?? 1000, model: "m", endpoint: "http://127.0.0.1:9" };
        fake.chatSettings = settings;
        return ok(settings);
      }),
  };
  return fake;
};
