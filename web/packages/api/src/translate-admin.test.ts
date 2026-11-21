import { describe, expect, it } from "vitest";
import { createdTokenFromWire, operationFromWire, settingsFromWire, systemStatusFromWire } from "./translate-admin";

describe("admin data from the wire", () => {
  it("reads a created token, its value only in `token`", () => {
    const out = createdTokenFromWire({ token: "secret", details: { id: "t", principal_id: "p", name: "ci", grants: [{ vault_id: "v", scope: "read" }], created_by: "p", created_at_ms: 1 } });
    expect(out).toEqual({ token: "secret", details: { id: "t", principalId: "p", name: "ci", grants: [{ vault: "v", scope: "read" }], createdBy: "p", createdAtMs: 1, expiresAtMs: null, lastUsedAtMs: null, revokedAtMs: null } });
  });
  it("reads an operation's kind from its input, with its progress, report and last error", () => {
    const job = operationFromWire({ id: "j", kind: "operation", state: "dead", input: { op: "reindex", index: "vector" } as never, max_attempts: 1, next_run_at_ms: 0, attempts: [{ error: "first" }, { error: "disk full" }] as never, created_at_ms: 2, progress: { step: 1, of: 2, label: "building" }, report: { done: true } as never });
    expect(job).toMatchObject({ op: "reindex", progress: { step: 1, of: 2, label: "building" }, report: { done: true }, error: "disk full" });
    expect(operationFromWire({ id: "j", kind: "operation", state: "queued", input: [] as never, max_attempts: 1, next_run_at_ms: 0, attempts: [], created_at_ms: 2 })).toMatchObject({ op: "operation", input: {}, report: null, progress: null, error: null });
  });
  it("reads a status with Postgres down and no vault figures", () => {
    const s = systemStatusFromWire({ version: "1", postgres: { configured: true, reachable: false, error: "refused" }, llm: { configured: false, reachable: false }, abandoned_handlers: 0, backends: null as never, models: [], thresholds: {}, caches: [], caller_from_memory: true, vaults: null, vaults_error: "no answer" });
    expect(s).toMatchObject({ postgres: { reachable: false, error: "refused", latencyMs: null }, vaults: null, vaultsError: "no answer", callerFromMemory: true, backends: {} });
  });
  it("keeps settings as a tree, and anything else as empty", () => {
    expect(settingsFromWire({ a: { b: 1 } })).toEqual({ a: { b: 1 } });
    expect(settingsFromWire("x")).toEqual({});
  });
});
