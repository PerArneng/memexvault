import { epochMs, vaultId } from "@memex/types";
import type { EntityDetail, EntitySummary, EntityType } from "@memex/types";
import { describe, expect, it } from "vitest";
import { createEngine } from "./engine";
import { DEFAULT_SETTINGS } from "./settings";
import { countingIds, fakeApi, httpFailure, manualClock, memoryDraftStorage, memoryPreferences, UNREACHABLE } from "./testing";
import type { EntitiesSeed } from "./testing-entities";

const vault = vaultId("v1");
const type = (name: string, version = 1): EntityType => ({ name, version, schema: {}, change: "seed", reasons: [], changesetId: null, createdAt: epochMs(0) });
const row = (id: string): EntitySummary => ({ id, typeName: "T", typeVersion: 1, version: 1, aliases: [id], attributes: {}, conflictedFields: [], mergedFrom: [], updatedAt: epochMs(0) });
const detail: EntityDetail = {
  id: "e1", typeName: "T", typeVersion: 1, version: 3, aliases: [], mergedFrom: [], redirectedFrom: null, changesetId: "cs", updatedAt: epochMs(0),
  fields: [{ field: "a", value: 1, why: "", current: { assertionId: "c1", value: 1, epistemic: null, machine: null, creator: null, changesetId: null, evidence: [] }, conflicts: [], agreeing: [] }],
};

const setup = (entities: EntitiesSeed = {}) => {
  const api = fakeApi({ entities });
  const engine = createEngine({ clock: manualClock(), settings: DEFAULT_SETTINGS, api, preferences: memoryPreferences(), drafts: memoryDraftStorage(), ids: countingIds() });
  return { api, engine };
};

const NEW = { type: "T", aliases: [], fields: [], rationale: "r" };

describe("entity cases", () => {
  it("lists every type across pages", async () => {
    const { engine } = setup({ types: [type("A"), type("B"), type("C")], pageSize: 2 });
    const result = await engine.listEntityTypes(vault);
    expect(result).toMatchObject({ ok: true, value: { truncated: false } });
    if (result.ok) expect(result.value.types.map((t) => t.name)).toEqual(["A", "B", "C"]);
  });

  it("sends filters in the API's form and pages by cursor", async () => {
    const { api, engine } = setup({ entities: [row("a"), row("b"), row("c")], pageSize: 2 });
    const first = await engine.listEntities(vault, "T", [{ field: "price", op: "lt", value: "100" }], null);
    expect(api.calls).toContain("listEntities:T:price:lt:100");
    expect(first.ok && first.value.nextCursor).toBe("2");
    const second = await engine.listEntities(vault, "T", [], "2");
    expect(second.ok && second.value.items.map((r) => r.id)).toEqual(["c"]);
  });

  it("reports what a proposal became, in the server's terms", async () => {
    const committed = setup({ entityWrite: "committed" });
    expect(await committed.engine.createEntity(vault, NEW)).toEqual({ kind: "saved", changesetId: "cs-new", entityId: "entity-new" });
    const pending = setup({ entityWrite: "pending" });
    expect(await pending.engine.createEntity(vault, NEW)).toEqual({ kind: "pending", changesetId: "cs-new", entityId: "entity-new" });
    const refused = setup({ entityWrite: "rejected" });
    expect(await refused.engine.createEntity(vault, NEW)).toEqual({ kind: "refused", reasons: ["refused by the fake"] });
  });

  it("has merges and splits wait for review, whatever a normal write does", async () => {
    const { engine } = setup({ entityWrite: "committed", details: { e1: detail } });
    expect((await engine.mergeEntity(vault, "e1", { into: "e2", rationale: "r" })).kind).toBe("pending");
    expect((await engine.splitEntity(vault, "e1", { baseVersion: 1, moved: [], aliases: [], fields: [], rationale: "r" })).kind).toBe("pending");
  });

  it("a breaking type change that comes back pending stays pending", async () => {
    const { engine } = setup({ typeWrite: "pending" });
    expect(await engine.submitEntityType(vault, { name: "T", baseVersion: 1, schema: {}, rationale: "r" })).toEqual({ kind: "pending", changesetId: "cs-type-T" });
  });

  it("a conflict is stale, and any other failure says what it was", async () => {
    const { api, engine } = setup();
    api.failNext(httpFailure(409, "stale_version"));
    expect((await engine.createEntity(vault, NEW)).kind).toBe("stale");
    api.failNext(httpFailure(403, "forbidden"));
    expect((await engine.createEntity(vault, NEW)).kind).toBe("failed");
  });

  it("reuses a write's key after no answer, so a retry can't apply twice", async () => {
    const { api, engine } = setup();
    api.failNext(UNREACHABLE);
    await engine.createEntity(vault, NEW);
    await engine.createEntity(vault, NEW);
    await engine.createEntity(vault, NEW);
    const keys = api.calls.filter((c) => c.startsWith("createEntity:"));
    // The retry uses the unanswered call's key. Once an answer has come, the next write gets its own.
    expect(keys).toHaveLength(3);
    expect(keys[0]).toBe(keys[1]);
    expect(keys[2]).not.toBe(keys[1]);
  });

  it("reads a claim's version just before replacing it", async () => {
    const { api, engine } = setup({ details: { e1: detail }, claimVersions: { c1: 7 } });
    const result = await engine.editEntity(vault, "e1", { baseVersion: 3, aliases: null, fields: [{ field: "a", value: 2, evidence: [], replaces: null }], replacing: { a: "c1" }, rationale: "r" });
    expect(result.kind).toBe("saved");
    expect(api.calls).toContain('editEntity:[{"assertionId":"c1","version":7}]');
  });

  it("refuses a replacement it can't name a version for, rather than adding beside it", async () => {
    const { api, engine } = setup({ details: { e1: detail } });
    const result = await engine.editEntity(vault, "e1", { baseVersion: 3, aliases: null, fields: [{ field: "a", value: 2, evidence: [], replaces: null }], replacing: { a: "c1" }, rationale: "r" });
    expect(result.kind).toBe("refused");
    expect(api.calls.some((c) => c.startsWith("editEntity"))).toBe(false);
  });

  it("asks about the entity and each of its claims for the graph", async () => {
    const { api, engine } = setup({ neighbors: { c1: [{ edgeId: "x", kind: "claim_ref", direction: "in", from: "p", to: "c1", neighbor: "p", title: "P" }] } });
    const result = await engine.entityNeighborhood(vault, detail);
    expect(api.calls).toEqual(expect.arrayContaining(["listNeighbors:e1", "listNeighbors:c1"]));
    expect(result.ok && result.value.answers.map((a) => [a.viaField, a.items.length])).toEqual([[null, 0], ["a", 1]]);
    expect(result.ok && result.value.failures).toBe(0);
  });

  it("says a graph is partial when one ask fails, and fails only when all do", async () => {
    const partial = setup();
    partial.api.failNext(httpFailure(500, "boom"));
    const some = await partial.engine.entityNeighborhood(vault, detail);
    expect(some.ok && some.value.failures).toBe(1);
    const none = setup();
    none.api.down = true;
    expect((await none.engine.entityNeighborhood(vault, detail)).ok).toBe(false);
  });

  it("notices a dead server on any entity read", async () => {
    const { api, engine } = setup();
    api.down = true;
    expect((await engine.getEntity(vault, "e1")).ok).toBe(false);
    expect(engine.connectivity().status).not.toBe("unknown");
  });
});
