import type { ErasureImpact, Identity, McpInfo, OperationJob, RowKey, Scope, SystemStatus, TokenRow, VaultId } from "@memex/types";
import { scopeIn, sectionPath } from "./navigation";

/**
 * The admin section's screens. `server` ones are a server admin's, as the API allows them no one else: a vault admin sees the vault's own
 * screens and a permission message on the rest, which the API would refuse anyway.
 */
export const ADMIN_TABS = [
  { id: "vault", label: "Vault", server: false },
  { id: "tokens", label: "Tokens", server: false },
  { id: "operations", label: "Operations", server: false },
  { id: "erasure", label: "Erasure", server: false },
  { id: "status", label: "Status", server: true },
  { id: "settings", label: "Settings", server: true },
  { id: "mcp", label: "MCP", server: true },
  { id: "vaults", label: "All vaults", server: true },
] as const;

export type AdminTabId = (typeof ADMIN_TABS)[number]["id"];
export type AdminTab = (typeof ADMIN_TABS)[number];

export const adminTabById = (id: string): AdminTab | null => ADMIN_TABS.find((t) => t.id === id) ?? null;

export type AdminRoute =
  | { readonly kind: "tab"; readonly tab: AdminTabId; readonly id: string | null }
  | { readonly kind: "unknown" };

/** `/vaults/<id>/admin/<tab>[/<id>]`, where the id is a source on the erasure screen and an operation on the operations screen. */
export const parseAdminRoute = (rest: readonly string[]): AdminRoute => {
  const [name, id, ...more] = rest;
  if (name === undefined) return { kind: "tab", tab: "vault", id: null };
  const tab = adminTabById(name);
  if (tab === null || more.length > 0) return { kind: "unknown" };
  return { kind: "tab", tab: tab.id, id: id ?? null };
};

export const adminPath = (vault: VaultId, tab: AdminTabId, id: string | null = null): string =>
  `${sectionPath(vault, "admin")}/${tab}${id === null ? "" : `/${encodeURIComponent(id)}`}`;

export type AdminAccess =
  | { readonly kind: "allowed" }
  /** A server-wide screen, for someone who isn't a server admin. */
  | { readonly kind: "needs_server_admin" }
  | { readonly kind: "needs_vault_admin"; readonly have: Scope | null };

/** The same rules as the server's: a server admin may do everything, and a vault admin what concerns that vault. */
export const adminAccess = (identity: Identity, vault: VaultId, tab: AdminTab): AdminAccess => {
  if (identity.principal.serverAdmin) return { kind: "allowed" };
  if (tab.server) return { kind: "needs_server_admin" };
  const have = scopeIn(identity, vault);
  return have === "admin" ? { kind: "allowed" } : { kind: "needs_vault_admin", have };
};

export const adminAccessText = (access: AdminAccess): string | null => {
  switch (access.kind) {
    case "allowed":
      return null;
    case "needs_server_admin":
      return "This screen is for server admins, and your principal isn't one. The server refuses these calls to anyone else too.";
    case "needs_vault_admin":
      return `This screen needs admin scope in this vault, and your token has ${access.have ?? "none"}. The server refuses these calls to anyone else too.`;
  }
};

/**
 * A destructive action runs only once the person has typed what it acts on, so no single click can do it. The match is exact, apart from
 * spaces around it, as near enough would let a wrong vault through.
 */
export const confirmationMatches = (expected: string, typed: string): boolean => expected.trim() !== "" && typed.trim() === expected.trim();

/** A name for a vault, principal or token: the server's own rule, so the form can say so before sending. */
export const nameProblem = (name: string): string | null => {
  const trimmed = name.trim();
  if (trimmed === "") return "A name is needed.";
  if (trimmed.length > 200) return "A name can be at most 200 characters.";
  return null;
};

export type OperationKind = "reindex" | "rollback" | "reconcile" | "export" | "sweep" | "compact";

export interface OperationDef {
  readonly op: OperationKind;
  readonly label: string;
  readonly summary: string;
  /** It takes `keyword` or `vector`. */
  readonly needsIndex: boolean;
  /** It changes or drops something that can't be had back by running it again. Asks for the vault's name first. */
  readonly destructive: boolean;
}

export const OPERATIONS: readonly OperationDef[] = [
  { op: "reindex", label: "Reindex", summary: "Builds a new generation of an index from the event log, and switches to it when it's done. Search keeps the old one until then.", needsIndex: true, destructive: false },
  { op: "rollback", label: "Roll back an index", summary: "Goes back to an index's previous generation, which then catches up from the log. The newer generation is dropped.", needsIndex: true, destructive: true },
  { op: "reconcile", label: "Reconcile", summary: "Replays the event log and compares it with the tables, blobs and indexes, and reports what differs. With repair, it rebuilds an index that differs.", needsIndex: false, destructive: false },
  { op: "export", label: "Export", summary: "Writes the whole vault to an archive this page can download, which an import restores.", needsIndex: false, destructive: false },
  { op: "sweep", label: "Sweep blobs", summary: "Deletes blob files nothing refers to that are older than the grace period.", needsIndex: false, destructive: true },
  { op: "compact", label: "Compact", summary: "Rewrites every table of the vault in Postgres so deleted data is gone from the bytes on disk. Each table is locked while it's rewritten, so writes wait.", needsIndex: false, destructive: true },
];

export const operationDef = (op: string): OperationDef | null => OPERATIONS.find((o) => o.op === op) ?? null;

export interface OperationRequest {
  readonly op: OperationKind;
  readonly index: "keyword" | "vector" | null;
  readonly deep: boolean;
  readonly repair: boolean;
  readonly at: number | null;
  readonly graceS: number | null;
}

export const operationRequest = (op: OperationKind): OperationRequest => ({ op, index: operationDef(op)?.needsIndex === true ? "keyword" : null, deep: false, repair: false, at: null, graceS: null });

/** Reconcile with repair rebuilds an index, so it asks like the destructive ones do. */
export const needsTypedConfirmation = (request: OperationRequest): boolean => operationDef(request.op)?.destructive === true || (request.op === "reconcile" && request.repair);

export const operationRequestProblem = (request: OperationRequest): string | null => {
  const def = operationDef(request.op);
  if (def === null) return "There's no such operation.";
  if (def.needsIndex && request.index === null) return "Choose an index.";
  if (request.at !== null && (!Number.isInteger(request.at) || request.at < 0)) return "The event to replay to must be a whole number, 0 or more.";
  if (request.graceS !== null && (!Number.isInteger(request.graceS) || request.graceS < 0)) return "The grace period must be a whole number of seconds, 0 or more.";
  return null;
};

/** What an operation's job is doing, in words. */
export const operationStateText = (job: OperationJob): string => {
  const step = job.progress === null ? "" : `step ${String(job.progress.step)} of ${String(job.progress.of)}${job.progress.label === "" ? "" : `: ${job.progress.label}`}`;
  switch (job.state) {
    case "queued":
      return "Waiting to start";
    case "running":
      return step === "" ? "Running" : `Running, ${step}`;
    case "done":
      return "Done";
    case "dead":
      return job.error === null ? "Failed" : `Failed: ${job.error}`;
    case "cancelled":
      return "Cancelled";
    default:
      return job.state;
  }
};

/** 0 to 1, or null when the job doesn't say. */
export const operationFraction = (job: OperationJob): number | null => {
  if (job.state === "done") return 1;
  if (job.progress === null || job.progress.of <= 0) return null;
  return Math.min(1, Math.max(0, (job.progress.step - (job.state === "running" ? 1 : 0)) / job.progress.of));
};

export const isOperationOpen = (job: OperationJob): boolean => job.state === "queued" || job.state === "running";

/** The settings markers spec 039 puts where a secret would be. */
export const SECRET_SET = "<set>";
export const SECRET_NOT_SET = "<not set>";
export const REDACTED = "<redacted>";

export type SettingKind = "plain" | "secret_set" | "secret_not_set" | "redacted_url" | "empty";

export interface SettingRow {
  readonly path: string;
  readonly value: string;
  readonly kind: SettingKind;
}

const settingKind = (value: unknown, text: string): SettingKind => {
  if (value === null || value === undefined) return "empty";
  if (text === SECRET_SET) return "secret_set";
  if (text === SECRET_NOT_SET) return "secret_not_set";
  if (text.includes(REDACTED)) return "redacted_url";
  return "plain";
};

const MAX_SETTING_ROWS = 2_000;

/**
 * The server's settings tree as rows of `a.b.c` and a value, so a new setting needs no change here. Arrays of plain values read as one
 * list, and arrays of objects get an index per item. Bounded, as it's the server's data.
 */
export const settingRows = (tree: unknown): readonly SettingRow[] => {
  const rows: SettingRow[] = [];
  const walk = (value: unknown, path: string, depth: number) => {
    if (rows.length >= MAX_SETTING_ROWS) return;
    if (depth < 8 && typeof value === "object" && value !== null) {
      if (Array.isArray(value)) {
        if (value.every((v) => typeof v !== "object" || v === null)) {
          const text = value.map((v) => String(v)).join(", ");
          rows.push({ path, value: text === "" ? "(none)" : text, kind: "plain" });
          return;
        }
        value.forEach((v, i) => walk(v, `${path}[${String(i)}]`, depth + 1));
        return;
      }
      const entries = Object.entries(value);
      if (entries.length === 0) rows.push({ path, value: "(none)", kind: "empty" });
      for (const [key, v] of entries) walk(v, path === "" ? key : `${path}.${key}`, depth + 1);
      return;
    }
    const text = value === null || value === undefined ? "(not set)" : typeof value === "string" ? value : JSON.stringify(value);
    rows.push({ path, value: text, kind: settingKind(value, text) });
  };
  walk(tree, "", 0);
  return rows;
};

export const settingKindText = (kind: SettingKind): string | null => {
  switch (kind) {
    case "secret_set":
      return "Secret, set. The server never sends its value.";
    case "secret_not_set":
      return "Secret, not set.";
    case "redacted_url":
      return "Credentials and parameters are hidden.";
    default:
      return null;
  }
};

export type HealthTone = "success" | "warning" | "danger" | "neutral";

export interface HealthLine {
  readonly id: string;
  readonly title: string;
  readonly detail: string;
  readonly tone: HealthTone;
}

const reachLine = (id: string, name: string, reach: SystemStatus["postgres"], downEffect: string): HealthLine => {
  if (!reach.configured) return { id, title: `${name} isn't configured`, detail: "It's off on this server.", tone: "neutral" };
  if (reach.reachable) return { id, title: `${name} is reachable`, detail: reach.latencyMs === null ? "" : `Answered in ${String(reach.latencyMs)} ms.`, tone: "success" };
  return { id, title: `${name} is down`, detail: `${reach.error ?? "No answer."} ${downEffect}`, tone: "danger" };
};

/** The page's summary of what's wrong and what it means, first the worst. */
export const healthLines = (status: SystemStatus): readonly HealthLine[] => {
  const lines: HealthLine[] = [
    reachLine("postgres", "Postgres", status.postgres, "Nothing can be read or written until it's back, and the console's other screens will fail. This page keeps working, as it doesn't need Postgres."),
    reachLine("llm", "The LLM", status.llm, "Extraction, review, audits and chat wait and retry until it's back. Search, reading and uploads still work."),
  ];
  if (status.callerFromMemory) lines.push({ id: "memory", title: "Signed in from memory", detail: "Postgres couldn't check your session, so the server used its check from the last 5 minutes. Only this page works like that.", tone: "warning" });
  if (status.vaults === null) lines.push({ id: "vaults", title: "Vault figures unavailable", detail: status.vaultsError ?? "They couldn't be read in time.", tone: "warning" });
  if (status.abandonedHandlers > 0) lines.push({ id: "abandoned", title: `${String(status.abandonedHandlers)} job handlers abandoned`, detail: "They ran past their time limit and were left running. A restart clears them.", tone: "warning" });
  const missing = status.models.filter((m) => m.status !== "ready" && m.status !== "off");
  for (const m of missing) lines.push({ id: `model-${m.role}`, title: `The ${m.role} model is ${m.status}`, detail: m.model ?? "", tone: "warning" });
  const rank: Readonly<Record<HealthTone, number>> = { danger: 0, warning: 1, neutral: 2, success: 3 };
  return [...lines].sort((a, b) => rank[a.tone] - rank[b.tone]);
};

/** A status read that failed, explained, since the page must say what's wrong rather than fail itself. */
export const statusFailureText = (failure: { readonly kind: string; readonly status?: number; readonly message: string }): string => {
  if (failure.kind === "unreachable") return "The memexvault server itself isn't answering. It may be stopped or restarting, or the network is down. This page tries again by itself.";
  if (failure.status === 503) return `The server is up, but couldn't check your session, most likely because Postgres is down: ${failure.message} This page tries again by itself, and once Postgres answers it shows everything again.`;
  return failure.message;
};

export const hitRate = (hits: number, misses: number): string => (hits + misses === 0 ? "–" : `${((100 * hits) / (hits + misses)).toFixed(1)}%`);

export type TokenState = "active" | "revoked" | "expired";

export const tokenState = (token: TokenRow, now: number): TokenState => {
  if (token.revokedAtMs !== null) return "revoked";
  if (token.expiresAtMs !== null && token.expiresAtMs <= now) return "expired";
  return "active";
};

const DAY_MS = 86_400_000;

/** An expiry typed as days from now. Blank is never. */
export const expiryFrom = (days: string, now: number): { readonly ok: true; readonly expiresAtMs: number | null } | { readonly ok: false; readonly problem: string } => {
  const trimmed = days.trim();
  if (trimmed === "") return { ok: true, expiresAtMs: null };
  const n = Number(trimmed);
  if (!Number.isInteger(n) || n < 1 || n > 3650) return { ok: false, problem: "Expiry must be a whole number of days from 1 to 3650, or blank for none." };
  return { ok: true, expiresAtMs: now + n * DAY_MS };
};

/** The JSON an MCP client takes to reach this server over HTTP. The token is a placeholder, as the console never holds one. */
export const mcpClientConfig = (origin: string, info: McpInfo): string =>
  JSON.stringify({ mcpServers: { memexvault: { type: "http", url: `${origin}${info.endpoint}`, headers: { Authorization: "Bearer <your token>" } } } }, null, 2);

export const mcpStdioConfig = (): string =>
  JSON.stringify({ mcpServers: { memexvault: { command: "memexvault", args: ["mcp"], env: { MEMEX_DATABASE_URL: "<the server's database URL>", MEMEX_MCP_TOKEN: "<your token>" } } } }, null, 2);

const keyText = (k: RowKey): string => `${k.table}:${k.id}`;

const diffList = (label: string, preview: readonly string[], result: readonly string[]): readonly string[] => {
  const before = new Set(preview);
  const after = new Set(result);
  return [...[...before].filter((k) => !after.has(k)).map((k) => `${label}: ${k} was in the preview but not the result`), ...[...after].filter((k) => !before.has(k)).map((k) => `${label}: ${k} is in the result but wasn't in the preview`)];
};

/** Whether what an erasure did is what its preview said, and if not, each difference. */
export const compareErasure = (preview: ErasureImpact, result: ErasureImpact): { readonly same: boolean; readonly differences: readonly string[] } => {
  const differences = [
    ...diffList("source", preview.sources, result.sources),
    ...diffList("removed", preview.removed.map(keyText), result.removed.map(keyText)),
    ...diffList("rewritten", preview.rewritten.map(keyText), result.rewritten.map(keyText)),
    ...diffList("flagged claim", preview.flaggedClaims, result.flaggedClaims),
    ...diffList("flagged page", preview.flaggedPages, result.flaggedPages),
  ];
  return { same: differences.length === 0, differences };
};

/** The removed rows counted per table, largest first. */
export const removedByTable = (impact: ErasureImpact): readonly { readonly table: string; readonly count: number }[] => {
  const counts = new Map<string, number>();
  for (const k of impact.removed) counts.set(k.table, (counts.get(k.table) ?? 0) + 1);
  return [...counts].map(([table, count]) => ({ table, count })).sort((a, b) => b.count - a.count || a.table.localeCompare(b.table));
};
