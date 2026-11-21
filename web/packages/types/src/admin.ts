import type { Grant } from "./index";

/** A principal as a server admin sees it in the registry. The name was chosen by whoever made it, so it's untrusted text. */
export interface PrincipalRow {
  readonly id: string;
  readonly name: string;
  readonly kind: "human" | "agent";
  readonly serverAdmin: boolean;
  readonly disabled: boolean;
  readonly createdBy: string;
  readonly createdAtMs: number;
}

/** A token's details. The token itself is never here: it's shown once, in `CreatedToken`. */
export interface TokenRow {
  readonly id: string;
  readonly principalId: string;
  readonly name: string;
  readonly grants: readonly Grant[];
  readonly createdBy: string;
  readonly createdAtMs: number;
  readonly expiresAtMs: number | null;
  readonly lastUsedAtMs: number | null;
  readonly revokedAtMs: number | null;
}

export interface CreatedToken {
  /** The secret, which the server never gives again. */
  readonly token: string;
  readonly details: TokenRow;
}

export interface RowKey {
  readonly table: string;
  readonly id: string;
}

/** What an erasure removes, rewrites and flags. A preview and the record of a done erasure have the same shape, so they can be compared. */
export interface ErasureImpact {
  readonly logicalId: string;
  readonly sources: readonly string[];
  readonly removed: readonly RowKey[];
  readonly rewritten: readonly RowKey[];
  readonly flaggedClaims: readonly string[];
  readonly flaggedPages: readonly string[];
}

/** A done erasure. */
export interface ErasureRecord {
  readonly seq: number;
  readonly timeMs: number;
  /** Who approved it. */
  readonly actor: string;
  readonly changesetId: string;
  readonly impact: ErasureImpact;
}

export interface JobProgress {
  readonly step: number;
  readonly of: number;
  readonly label: string;
}

/** An operation's job, with the progress and report the jobs API carries for it. */
export interface OperationJob {
  readonly id: string;
  /** `reindex`, `rollback`, `reconcile`, `export`, `sweep`, `compact`, or a newer server's word. */
  readonly op: string;
  readonly input: Readonly<Record<string, unknown>>;
  /** `queued`, `running`, `done`, `dead` or `cancelled`. */
  readonly state: string;
  readonly progress: JobProgress | null;
  readonly report: Readonly<Record<string, unknown>> | null;
  readonly createdAtMs: number;
  readonly finishedAtMs: number | null;
  /** The last attempt's error, if it had one. */
  readonly error: string | null;
}

export interface ImportRun {
  readonly id: string;
  /** `running`, `done` or `failed`. */
  readonly state: string;
  readonly progress: JobProgress;
  readonly report: Readonly<Record<string, unknown>> | null;
  readonly error: string | null;
  readonly startedAtMs: number;
  readonly finishedAtMs: number | null;
}

export interface Reach {
  readonly configured: boolean;
  readonly reachable: boolean;
  readonly latencyMs: number | null;
  readonly error: string | null;
}

export interface IndexStatus {
  readonly ready: boolean;
  readonly building: boolean;
  readonly lag: number;
  readonly watermark: number | null;
}

export interface VaultStatus {
  readonly id: string;
  readonly name: string;
  readonly head: number | null;
  readonly keyword: IndexStatus;
  readonly vector: IndexStatus;
  readonly openJobs: readonly { readonly kind: string; readonly count: number }[];
  readonly deadJobs: number;
  readonly reviewPending: number;
  readonly blobs: number;
  readonly blobBytes: number;
}

export interface CacheTier {
  readonly tier: string;
  readonly capBytes: number;
  readonly sizeBytes: number;
  readonly entries: number;
  readonly hits: number;
  readonly misses: number;
  readonly evictions: number;
  readonly rejected: number;
  readonly corrupt: number;
}

export interface ModelStatus {
  readonly role: string;
  readonly model: string | null;
  readonly status: string;
}

export interface SystemStatus {
  readonly version: string;
  readonly postgres: Reach;
  readonly llm: Reach;
  readonly abandonedHandlers: number;
  readonly memoryRssBytes: number | null;
  readonly models: readonly ModelStatus[];
  readonly caches: readonly CacheTier[];
  /** Null when they couldn't be read in time, with the reason in `vaultsError`. */
  readonly vaults: readonly VaultStatus[] | null;
  readonly vaultsError: string | null;
  /** Postgres couldn't check the caller, and a recent check of the same credential was used. */
  readonly callerFromMemory: boolean;
  readonly backends: Readonly<Record<string, unknown>>;
  readonly thresholds: Readonly<Record<string, unknown>>;
}

export interface UsageRow {
  readonly agent: string;
  readonly model: string;
  readonly vaultId: string;
  readonly day: string;
  readonly derivations: number;
  readonly inputTokens: number;
  readonly outputTokens: number;
  readonly thinkingTokens: number;
  /** Only where a price is configured. Never guessed. */
  readonly costUsd: number | null;
}

export interface Usage {
  readonly rows: readonly UsageRow[];
  readonly pricedCostUsd: number;
  readonly unpricedTokens: number;
}

export interface McpInfo {
  readonly endpoint: string;
  readonly transports: readonly string[];
  readonly protocolVersion: string;
  readonly supportedProtocolVersions: readonly string[];
  readonly allowedHosts: readonly string[];
  readonly tools: readonly { readonly name: string; readonly description: string }[];
}

export interface ClearedCaches {
  readonly vaults: number;
  readonly caches: readonly CacheTier[];
}
