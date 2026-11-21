import { vaultId } from "@memex/types";
import type { CacheTier, ClearedCaches, CreatedToken, ErasureImpact, ErasureRecord, ImportRun, IndexStatus, JobProgress, McpInfo, OperationJob, Page, PrincipalRow, Reach, SystemStatus, TokenRow, Usage, VaultStatus } from "@memex/types";
import type { components } from "./generated/schema";

type Schemas = components["schemas"];

const isObject = (value: unknown): value is Record<string, unknown> => typeof value === "object" && value !== null && !Array.isArray(value);
const objectOrNull = (value: unknown): Readonly<Record<string, unknown>> | null => (isObject(value) ? value : null);

export const principalFromWire = (p: Schemas["PrincipalDto"]): PrincipalRow => ({
  id: p.id,
  name: p.name,
  kind: p.kind,
  serverAdmin: p.server_admin,
  disabled: p.disabled,
  createdBy: p.created_by,
  createdAtMs: p.created_at_ms,
});

export const principalPageFromWire = (p: Schemas["PrincipalPage"]): Page<PrincipalRow> => ({ items: p.items.map(principalFromWire), nextCursor: p.next_cursor ?? null });

export const tokenFromWire = (t: Schemas["TokenDto"]): TokenRow => ({
  id: t.id,
  principalId: t.principal_id,
  name: t.name,
  grants: t.grants.map((g) => ({ vault: vaultId(g.vault_id), scope: g.scope })),
  createdBy: t.created_by,
  createdAtMs: t.created_at_ms,
  expiresAtMs: t.expires_at_ms ?? null,
  lastUsedAtMs: t.last_used_at_ms ?? null,
  revokedAtMs: t.revoked_at_ms ?? null,
});

export const tokenPageFromWire = (p: Schemas["TokenPage"]): Page<TokenRow> => ({ items: p.items.map(tokenFromWire), nextCursor: p.next_cursor ?? null });

export const createdTokenFromWire = (c: Schemas["CreatedTokenDto"]): CreatedToken => ({ token: c.token, details: tokenFromWire(c.details) });

export const erasureImpactFromWire = (i: Schemas["ErasureImpactDto"]): ErasureImpact => ({
  logicalId: i.logical_id,
  sources: i.sources,
  removed: i.removed.map((r) => ({ table: r.table, id: r.id })),
  rewritten: i.rewritten.map((r) => ({ table: r.table, id: r.id })),
  flaggedClaims: i.flagged_claims,
  flaggedPages: i.flagged_pages,
});

export const erasurePageFromWire = (p: Schemas["ErasurePage"]): Page<ErasureRecord> => ({
  items: p.items.map((e) => ({ seq: e.seq, timeMs: e.time_ms, actor: e.actor, changesetId: e.changeset_id, impact: erasureImpactFromWire(e.impact) })),
  nextCursor: p.next_cursor ?? null,
});

const progressFromWire = (p: Schemas["JobProgressDto"]): JobProgress => ({ step: p.step, of: p.of, label: p.label });

/** The last attempt's error, as the API documents an attempt only as an object. */
const lastError = (attempts: readonly unknown[]): string | null => {
  const last = attempts.at(-1);
  return isObject(last) && typeof last.error === "string" ? last.error : null;
};

export const operationFromWire = (j: Schemas["JobDto"]): OperationJob => {
  const input = objectOrNull(j.input) ?? {};
  return {
    id: j.id,
    op: typeof input.op === "string" ? input.op : j.kind,
    input,
    state: j.state,
    progress: j.progress === null || j.progress === undefined ? null : progressFromWire(j.progress),
    report: objectOrNull(j.report),
    createdAtMs: j.created_at_ms,
    finishedAtMs: j.finished_at_ms ?? null,
    error: lastError(j.attempts),
  };
};

export const operationPageFromWire = (p: Schemas["JobPage"]): Page<OperationJob> => ({ items: p.items.map(operationFromWire), nextCursor: p.next_cursor ?? null });

export const importFromWire = (i: Schemas["ImportDto"]): ImportRun => ({
  id: i.id,
  state: i.state,
  progress: progressFromWire(i.progress),
  report: objectOrNull(i.report),
  error: i.error ?? null,
  startedAtMs: i.started_at_ms,
  finishedAtMs: i.finished_at_ms ?? null,
});

const reachFromWire = (r: Schemas["ReachDto"]): Reach => ({ configured: r.configured, reachable: r.reachable, latencyMs: r.latency_ms ?? null, error: r.error ?? null });

const indexFromWire = (i: Schemas["IndexStatusDto"]): IndexStatus => ({ ready: i.ready, building: i.building, lag: i.lag, watermark: i.watermark ?? null });

const vaultStatusFromWire = (v: Schemas["VaultStatusDto"]): VaultStatus => ({
  id: v.id,
  name: v.name,
  head: v.head ?? null,
  keyword: indexFromWire(v.keyword),
  vector: indexFromWire(v.vector),
  openJobs: v.open_jobs.map((j) => ({ kind: j.kind, count: j.count })),
  deadJobs: v.dead_jobs,
  reviewPending: v.review_pending,
  blobs: v.blobs,
  blobBytes: v.blob_bytes,
});

export const cacheTierFromWire = (c: Schemas["CacheTierDto"]): CacheTier => ({
  tier: c.tier,
  capBytes: c.cap_bytes,
  sizeBytes: c.size_bytes,
  entries: c.entries,
  hits: c.hits,
  misses: c.misses,
  evictions: c.evictions,
  rejected: c.rejected,
  corrupt: c.corrupt,
});

export const systemStatusFromWire = (s: Schemas["SystemStatusDto"]): SystemStatus => ({
  version: s.version,
  postgres: reachFromWire(s.postgres),
  llm: reachFromWire(s.llm),
  abandonedHandlers: s.abandoned_handlers,
  memoryRssBytes: s.memory_rss_bytes ?? null,
  models: s.models.map((m) => ({ role: m.role, model: m.model ?? null, status: m.status })),
  caches: s.caches.map(cacheTierFromWire),
  vaults: s.vaults === null || s.vaults === undefined ? null : s.vaults.map(vaultStatusFromWire),
  vaultsError: s.vaults_error ?? null,
  callerFromMemory: s.caller_from_memory,
  backends: objectOrNull(s.backends) ?? {},
  thresholds: objectOrNull(s.thresholds) ?? {},
});

export const usageFromWire = (u: Schemas["UsageDto"]): Usage => ({
  rows: u.rows.map((r) => ({
    agent: r.agent,
    model: r.model,
    vaultId: r.vault_id,
    day: r.day,
    derivations: r.derivations,
    inputTokens: r.input_tokens,
    outputTokens: r.output_tokens,
    thinkingTokens: r.thinking_tokens,
    costUsd: r.cost_usd ?? null,
  })),
  pricedCostUsd: u.priced_cost_usd,
  unpricedTokens: u.unpriced_tokens,
});

export const mcpInfoFromWire = (m: Schemas["McpInfoDto"]): McpInfo => ({
  endpoint: m.endpoint,
  transports: m.transports,
  protocolVersion: m.protocol_version,
  supportedProtocolVersions: m.supported_protocol_versions,
  allowedHosts: m.allowed_hosts,
  tools: m.tools.map((t) => ({ name: t.name, description: t.description })),
});

export const clearedCachesFromWire = (c: Schemas["ClearedCachesDto"]): ClearedCaches => ({ vaults: c.vaults, caches: c.caches.map(cacheTierFromWire) });

/** The settings are a tree the server decides, so they stay as data and a pure function in `core` lays them out. */
export const settingsFromWire = (s: unknown): Readonly<Record<string, unknown>> => objectOrNull(s) ?? {};
