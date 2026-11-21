import { vaultId } from "@memex/types";
import type { ErasureImpact, McpInfo, OperationJob, SystemStatus, TokenRow } from "@memex/types";
import { describe, expect, it } from "vitest";
import {
  ADMIN_TABS,
  adminAccess,
  adminAccessText,
  adminPath,
  adminTabById,
  compareErasure,
  confirmationMatches,
  expiryFrom,
  healthLines,
  hitRate,
  mcpClientConfig,
  nameProblem,
  needsTypedConfirmation,
  operationFraction,
  operationRequest,
  operationRequestProblem,
  operationStateText,
  OPERATIONS,
  parseAdminRoute,
  removedByTable,
  settingRows,
  statusFailureText,
  tokenState,
} from "./admin";
import { EMPTY_STATUS, identityWith } from "./testing";

const v = vaultId("v1");
const tab = (id: string) => {
  const t = adminTabById(id);
  if (t === null) throw new Error(id);
  return t;
};

describe("admin routes", () => {
  it("reads each tab, its object, and the default", () => {
    expect(parseAdminRoute([])).toEqual({ kind: "tab", tab: "vault", id: null });
    expect(parseAdminRoute(["erasure", "src-1"])).toEqual({ kind: "tab", tab: "erasure", id: "src-1" });
    expect(parseAdminRoute(["nope"])).toEqual({ kind: "unknown" });
    expect(parseAdminRoute(["status", "a", "b"])).toEqual({ kind: "unknown" });
    for (const t of ADMIN_TABS) expect(parseAdminRoute(adminPath(v, t.id).split("/").slice(4))).toEqual({ kind: "tab", tab: t.id, id: null });
    expect(adminPath(v, "erasure", "a/b")).toBe("/vaults/v1/admin/erasure/a%2Fb");
  });
});

describe("who may open each admin screen", () => {
  const vaultAdmin = identityWith([{ vault: v, scope: "admin" }]);
  const reader = identityWith([{ vault: v, scope: "read" }]);
  const serverAdmin = identityWith([], true);
  it("a server admin may open them all", () => {
    for (const t of ADMIN_TABS) expect(adminAccess(serverAdmin, v, t).kind).toBe("allowed");
  });
  it("a vault admin the vault's own, and is told the server's need a server admin", () => {
    for (const t of ADMIN_TABS) expect(adminAccess(vaultAdmin, v, t).kind).toBe(t.server ? "needs_server_admin" : "allowed");
    expect(adminAccessText(adminAccess(vaultAdmin, v, tab("status")))).toMatch(/server admins/);
  });
  it("anyone less is refused every one, with what they have", () => {
    for (const t of ADMIN_TABS) expect(adminAccess(reader, v, t).kind).not.toBe("allowed");
    expect(adminAccessText(adminAccess(reader, v, tab("vault")))).toMatch(/your token has read/);
    expect(adminAccess(vaultAdmin, vaultId("other"), tab("tokens"))).toEqual({ kind: "needs_vault_admin", have: null });
  });
});

describe("typed confirmation", () => {
  it("needs exactly the name, and never matches an empty one", () => {
    expect(confirmationMatches("Fleet", "Fleet")).toBe(true);
    expect(confirmationMatches("Fleet", "  Fleet ")).toBe(true);
    expect(confirmationMatches("Fleet", "fleet")).toBe(false);
    expect(confirmationMatches("Fleet", "Flee")).toBe(false);
    expect(confirmationMatches("", "")).toBe(false);
    expect(confirmationMatches(" ", " ")).toBe(false);
  });
  it("names", () => {
    expect(nameProblem("  ")).toMatch(/needed/);
    expect(nameProblem("x".repeat(201))).toMatch(/200/);
    expect(nameProblem("ok")).toBeNull();
  });
});

const job = (over: Partial<OperationJob>): OperationJob => ({ id: "j", op: "reindex", input: {}, state: "queued", progress: null, report: null, createdAtMs: 0, finishedAtMs: null, error: null, ...over });

describe("operations", () => {
  it("destructive ones and a repairing reconcile ask for the name, the others don't", () => {
    const destructive = OPERATIONS.filter((o) => needsTypedConfirmation(operationRequest(o.op))).map((o) => o.op);
    expect(destructive).toEqual(["rollback", "sweep", "compact"]);
    expect(needsTypedConfirmation({ ...operationRequest("reconcile"), repair: true })).toBe(true);
  });
  it("checks what it sends", () => {
    expect(operationRequestProblem(operationRequest("reindex"))).toBeNull();
    expect(operationRequestProblem({ ...operationRequest("reindex"), index: null })).toMatch(/index/);
    expect(operationRequestProblem({ ...operationRequest("reconcile"), at: -1 })).toMatch(/whole number/);
    expect(operationRequestProblem({ ...operationRequest("sweep"), graceS: 1.5 })).toMatch(/grace/);
    expect(operationRequestProblem({ ...operationRequest("reindex"), op: "melt" as never })).toMatch(/no such/);
  });
  it("says where a job is", () => {
    expect(operationStateText(job({ state: "running", progress: { step: 1, of: 2, label: "building" } }))).toBe("Running, step 1 of 2: building");
    expect(operationStateText(job({ state: "dead", error: "disk full" }))).toBe("Failed: disk full");
    expect(operationStateText(job({ state: "weird" }))).toBe("weird");
    expect(operationFraction(job({ state: "running", progress: { step: 2, of: 4, label: "" } }))).toBe(0.25);
    expect(operationFraction(job({ state: "done" }))).toBe(1);
    expect(operationFraction(job({ state: "queued" }))).toBeNull();
    expect(operationFraction(job({ state: "running", progress: { step: 1, of: 0, label: "" } }))).toBeNull();
  });
});

describe("settings", () => {
  const rows = settingRows({
    database_url: "postgres://<redacted>@db:5432/memex",
    llm: { provider: "gemini", api_key: "<set>" },
    mcp_token: "<not set>",
    mcp_allowed_hosts: ["localhost", "127.0.0.1"],
    llm_prices: [{ model: "m", input: 1 }],
    reviewer_model: null,
    empty: {},
    thresholds: { auto_commit: 0.95 },
  });
  it("flattens the tree, so a new setting needs nothing here", () => {
    expect(rows.map((r) => r.path)).toEqual(["database_url", "llm.provider", "llm.api_key", "mcp_token", "mcp_allowed_hosts", "llm_prices[0].model", "llm_prices[0].input", "reviewer_model", "empty", "thresholds.auto_commit"]);
    expect(rows.find((r) => r.path === "mcp_allowed_hosts")?.value).toBe("localhost, 127.0.0.1");
    expect(rows.find((r) => r.path === "thresholds.auto_commit")?.value).toBe("0.95");
  });
  it("marks spec 039's secret markers", () => {
    const kind = (path: string) => rows.find((r) => r.path === path)?.kind;
    expect(kind("llm.api_key")).toBe("secret_set");
    expect(kind("mcp_token")).toBe("secret_not_set");
    expect(kind("database_url")).toBe("redacted_url");
    expect(kind("reviewer_model")).toBe("empty");
    expect(kind("llm.provider")).toBe("plain");
  });
  it("stays bounded on a hostile tree", () => {
    let deep: unknown = "x";
    for (let i = 0; i < 50; i += 1) deep = { a: deep };
    expect(settingRows(deep).length).toBe(1);
    expect(settingRows({ many: Array.from({ length: 5000 }, (_, i) => ({ i })) }).length).toBe(2000);
    expect(settingRows(null)).toEqual([{ path: "", value: "(not set)", kind: "empty" }]);
  });
});

describe("status", () => {
  const down: SystemStatus = { ...EMPTY_STATUS, postgres: { configured: true, reachable: false, latencyMs: null, error: "connection refused" }, vaults: null, vaultsError: "no answer within 1500 ms", callerFromMemory: true };
  it("puts Postgres down first and explains what it means", () => {
    const lines = healthLines(down);
    expect(lines[0]).toMatchObject({ id: "postgres", tone: "danger" });
    expect(lines[0]?.detail).toMatch(/connection refused.*Nothing can be read or written/);
    expect(lines.map((l) => l.id)).toEqual(expect.arrayContaining(["memory", "vaults"]));
  });
  it("says an LLM that's off is off, not down", () => {
    expect(healthLines(EMPTY_STATUS).find((l) => l.id === "llm")).toMatchObject({ tone: "neutral", title: "The LLM isn't configured" });
    expect(healthLines(EMPTY_STATUS).find((l) => l.id === "postgres")).toMatchObject({ tone: "success", detail: "Answered in 3 ms." });
  });
  it("lists models that aren't ready, and abandoned handlers", () => {
    const lines = healthLines({ ...EMPTY_STATUS, abandonedHandlers: 2, models: [{ role: "reranker", model: null, status: "missing" }, { role: "embedder", model: "e5", status: "ready" }] });
    expect(lines.map((l) => l.id)).toEqual(expect.arrayContaining(["abandoned", "model-reranker"]));
    expect(lines.some((l) => l.id === "model-embedder")).toBe(false);
  });
  it("explains a failed read instead of failing", () => {
    expect(statusFailureText({ kind: "unreachable", message: "x" })).toMatch(/isn't answering/);
    expect(statusFailureText({ kind: "http", status: 503, message: "Postgres didn't answer." })).toMatch(/most likely because Postgres is down/);
    expect(hitRate(0, 0)).toBe("–");
    expect(hitRate(3, 1)).toBe("75.0%");
  });
});

describe("tokens", () => {
  const row = (over: Partial<TokenRow>): TokenRow => ({ id: "t", principalId: "p", name: "n", grants: [], createdBy: "p", createdAtMs: 0, expiresAtMs: null, lastUsedAtMs: null, revokedAtMs: null, ...over });
  it("is revoked, expired or active", () => {
    expect(tokenState(row({ revokedAtMs: 5 }), 10)).toBe("revoked");
    expect(tokenState(row({ expiresAtMs: 10 }), 10)).toBe("expired");
    expect(tokenState(row({ expiresAtMs: 11 }), 10)).toBe("active");
  });
  it("reads an expiry in days", () => {
    expect(expiryFrom("", 0)).toEqual({ ok: true, expiresAtMs: null });
    expect(expiryFrom("2", 1000)).toEqual({ ok: true, expiresAtMs: 1000 + 2 * 86_400_000 });
    for (const bad of ["0", "1.5", "-3", "x", "4000"]) expect(expiryFrom(bad, 0).ok).toBe(false);
  });
});

describe("MCP", () => {
  const info: McpInfo = { endpoint: "/api/v1/mcp", transports: ["streamable-http", "stdio"], protocolVersion: "2025-06-18", supportedProtocolVersions: [], allowedHosts: [], tools: [] };
  it("gives a client config with the server's address and no token", () => {
    const config = JSON.parse(mcpClientConfig("http://127.0.0.1:8080", info)) as { mcpServers: { memexvault: { url: string; headers: { Authorization: string } } } };
    expect(config.mcpServers.memexvault.url).toBe("http://127.0.0.1:8080/api/v1/mcp");
    expect(config.mcpServers.memexvault.headers.Authorization).toBe("Bearer <your token>");
  });
});

describe("erasure", () => {
  const preview: ErasureImpact = { logicalId: "s", sources: ["s"], removed: [{ table: "segment", id: "g1" }, { table: "source", id: "s" }, { table: "segment", id: "g2" }], rewritten: [{ table: "entity", id: "e" }], flaggedClaims: ["c"], flaggedPages: [] };
  it("matches a result with the same rows in any order", () => {
    expect(compareErasure(preview, { ...preview, removed: [...preview.removed].reverse() })).toEqual({ same: true, differences: [] });
  });
  it("names every difference", () => {
    const out = compareErasure(preview, { ...preview, removed: preview.removed.slice(1), flaggedPages: ["p"] });
    expect(out.same).toBe(false);
    expect(out.differences).toEqual(["removed: segment:g1 was in the preview but not the result", "flagged page: p is in the result but wasn't in the preview"]);
  });
  it("counts what goes per table", () => {
    expect(removedByTable(preview)).toEqual([{ table: "segment", count: 2 }, { table: "source", count: 1 }]);
  });
});
