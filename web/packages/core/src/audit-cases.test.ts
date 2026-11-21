import { vaultId } from "@memex/types";
import { describe, expect, it } from "vitest";
import { failed } from "./api";
import { createEngine } from "./engine";
import { DEFAULT_SETTINGS } from "./settings";
import { checkWith, countingIds, fakeApi, httpFailure, manualClock, memoryDraftStorage, memoryPreferences, runWith, UNREACHABLE, usageRowWith } from "./testing";
import type { FakeApi, FakeApiSeed } from "./testing";

const vault = vaultId("v");
const start = (audit: FakeApiSeed["audit"], change: (api: FakeApi) => void = () => undefined) => {
  const api = fakeApi({ audit: audit ?? {} });
  change(api);
  const engine = createEngine({ clock: manualClock(), settings: DEFAULT_SETTINGS, api, preferences: memoryPreferences(), drafts: memoryDraftStorage(), ids: countingIds() });
  return { api, engine };
};

describe("audit use cases", () => {
  it("reads every page of a run's checks, in rank order", async () => {
    const checks = Array.from({ length: 5 }, (_, i) => checkWith({ id: `c${String(i)}`, rank: i, outcome: i % 2 === 0 ? "passed" : "failed" }));
    const { engine } = start({ runs: [runWith()], checks: { "run-1": checks }, checkPageSize: 2 });
    const all = await engine.listAuditChecks(vault, "run-1", false);
    expect(all.ok && all.value.map((c) => c.rank)).toEqual([0, 1, 2, 3, 4]);
    const findings = await engine.listAuditChecks(vault, "run-1", true);
    expect(findings.ok && findings.value.map((c) => c.id)).toEqual(["c1", "c3"]);
  });
  it("starts a run with a seed, and the same seed again gives a run with that seed", async () => {
    const { engine } = start({});
    const first = await engine.triggerAudit(vault, { seed: "42", sampleSize: 5, runTokenBudget: null });
    const second = await engine.triggerAudit(vault, { seed: "42", sampleSize: 5, runTokenBudget: null });
    expect(first.ok && second.ok && [first.value.seed, second.value.seed, first.value.state]).toEqual(["42", "42", "queued"]);
    const runs = await engine.listAuditRuns(vault);
    expect(runs.ok && runs.value.map((r) => r.id)).toEqual(["run-2", "run-1"]);
  });
  it("passes a refusal through as data", async () => {
    const { engine } = start({ readOnly: true });
    const out = await engine.triggerAudit(vault, { seed: null, sampleSize: null, runTokenBudget: null });
    expect(out).toMatchObject({ ok: false, failure: { status: 403 } });
    const bad = await start({}).engine.putAuditSchedule(vault, { enabled: true, intervalMs: 1000, sampleSize: 1, runTokenBudget: 0, dayTokenBudget: 0, ttlDays: {} });
    expect(bad).toMatchObject({ ok: false, failure: { code: "invalid", message: "the interval must be at least a minute" } });
  });
  it("stops at the first page that fails, and a server that's away is noticed", async () => {
    const { engine } = start({ runs: [runWith()], checks: { "run-1": [checkWith(), checkWith({ id: "c2" })] }, checkPageSize: 1 }, (api) => {
      const real = api.listAuditChecks;
      api.listAuditChecks = (v, r, f, cursor) => (cursor === null ? real(v, r, f, cursor) : Promise.resolve(failed(UNREACHABLE)));
    });
    expect(await engine.listAuditChecks(vault, "run-1", false)).toMatchObject({ ok: false, failure: { kind: "unreachable" } });
    expect(engine.connectivity()).toMatchObject({ status: "online", failures: 1 });
  });
  it("can't loop for ever on cursors that never end", async () => {
    let calls = 0;
    const { engine } = start({}, (api) => {
      api.listAuditChecks = () => {
        calls += 1;
        return Promise.resolve({ ok: true, value: { items: [checkWith()], nextCursor: "again" } });
      };
    });
    const out = await engine.listAuditChecks(vault, "r", false);
    expect(out.ok && out.value).toHaveLength(10);
    expect(calls).toBe(10);
  });
  it("reads the usage report, and a non-admin's refusal comes back as a failure", async () => {
    const usage = { rows: [usageRowWith()], pricedCostUsd: 0, unpricedTokens: 110 };
    expect(await start({ usage }).engine.getAgentUsage("2026-11-01", "2026-11-30")).toEqual({ ok: true, value: usage });
    expect(await start({}).engine.getAgentUsage("2026-11-01", "2026-11-30")).toMatchObject({ ok: false, failure: httpFailure(403, "forbidden", "needs admin scope") });
  });
});
