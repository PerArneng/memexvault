import { vaultId } from "@memex/types";
import type { OperationJob, PrincipalRow } from "@memex/types";
import { describe, expect, it } from "vitest";
import { operationRequest } from "./admin";
import { createEngine } from "./engine";
import { DEFAULT_SETTINGS } from "./settings";
import { countingIds, fakeApi, httpFailure, manualClock, memoryDraftStorage, memoryPreferences, UNREACHABLE } from "./testing";
import type { FakeApiSeed } from "./testing";

const vault = vaultId("v");
const start = (admin: FakeApiSeed["admin"] = {}) => {
  const api = fakeApi({ admin });
  const engine = createEngine({ clock: manualClock(), settings: DEFAULT_SETTINGS, api, preferences: memoryPreferences(), drafts: memoryDraftStorage(), ids: countingIds() });
  return { api, engine };
};
const principal = (id: string): PrincipalRow => ({ id, name: id, kind: "human", serverAdmin: false, disabled: false, createdBy: "p1", createdAtMs: 0 });
const job = (id: string, createdAtMs: number): OperationJob => ({ id, op: "export", input: {}, state: "done", progress: null, report: null, createdAtMs, finishedAtMs: null, error: null });

describe("admin cases", () => {
  it("reads every page of principals and tokens", async () => {
    const { api, engine } = start({ principals: ["a", "b", "c", "d", "e"].map(principal) });
    const all = await engine.listPrincipals();
    expect(all.ok && all.value.map((p) => p.id)).toEqual(["a", "b", "c", "d", "e"]);
    expect(api.calls.filter((c) => c === "listPrincipals").length).toBe(3);
  });
  it("lists operations newest first, across pages", async () => {
    const { engine } = start({ operations: [job("a", 1), job("b", 3), job("c", 2)] });
    const out = await engine.listOperations(vault);
    expect(out.ok && out.value.map((o) => o.id)).toEqual(["b", "c", "a"]);
  });
  it("trims names before sending them", async () => {
    const { api, engine } = start();
    await engine.createVault("  Fleet  ");
    await engine.createToken({ name: " ci ", principalId: null, grants: [], expiresAtMs: null });
    expect(api.calls).toEqual(expect.arrayContaining(["createVault:Fleet", "createToken:ci"]));
  });
  it("gives a created token's value once, and no list carries it", async () => {
    const { api, engine } = start();
    const made = await engine.createToken({ name: "ci", principalId: null, grants: [{ vault, scope: "read" }], expiresAtMs: null });
    expect(made.ok && made.value.token).toBe(api.admin.secrets[0]);
    const listed = await engine.listTokens(true);
    expect(JSON.stringify(listed)).not.toContain(api.admin.secrets[0]);
  });
  it("a second operation of the same kind is refused, as the server does", async () => {
    const { engine } = start();
    expect((await engine.startOperation(vault, operationRequest("export"))).ok).toBe(true);
    const again = await engine.startOperation(vault, operationRequest("export"));
    expect(!again.ok && again.failure).toMatchObject({ status: 409 });
  });
  it("a server out of reach marks the connection, and a 403 is passed on", async () => {
    const { api, engine } = start();
    api.failNext(UNREACHABLE);
    expect((await engine.getSystemStatus()).ok).toBe(false);
    expect(engine.connectivity()).toEqual({ status: "online", failures: 1 });
    api.failNext(httpFailure(403, "forbidden"));
    const refused = await engine.getSystemSettings();
    expect(!refused.ok && refused.failure).toMatchObject({ status: 403 });
  });
  it("proposes an erasure, which waits for a person", async () => {
    const { engine } = start({ previews: { s: { logicalId: "s", sources: ["s"], removed: [], rewritten: [], flaggedClaims: [], flaggedPages: [] } } });
    const out = await engine.proposeErasure(vault, "s", "asked to");
    expect(out.ok && out.value).toMatchObject({ state: "pending", changesetId: "cs-erase-s" });
    expect((await engine.proposeErasure(vault, "missing", "x")).ok).toBe(false);
  });
});
