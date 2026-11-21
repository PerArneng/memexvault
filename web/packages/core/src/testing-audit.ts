import { epochMs, vaultId } from "@memex/types";
import type { AgentUsageReport, AgentUsageRow, AuditCheck, AuditRun, AuditSchedule, RecallReport } from "@memex/types";
import { failed, ok } from "./api";
import type { ApiFailure } from "./api";
import type { AuditApi } from "./audit-cases";

const failure = (status: number, code: string, message: string): ApiFailure => ({ kind: "http", status, code, message, requestId: "req-1" });

export interface AuditSeed {
  readonly runs?: readonly AuditRun[];
  readonly checks?: Readonly<Record<string, readonly AuditCheck[]>>;
  readonly schedule?: AuditSchedule;
  readonly recall?: RecallReport;
  readonly usage?: AgentUsageReport;
  /** Checks per page, to exercise cursors. */
  readonly checkPageSize?: number;
  /** Refuses starting runs and writing the schedule, as for a read token. */
  readonly readOnly?: boolean;
}

export const runWith = (over: Partial<AuditRun> = {}): AuditRun => ({
  id: "run-1",
  seed: "42",
  trigger: "on_demand",
  requestedBy: "pat",
  state: "finished",
  stop: null,
  stopDetail: null,
  sampleSize: 10,
  population: 100,
  sampled: 10,
  runTokenBudget: 1000,
  tokens: { input: 200, output: 50, thinking: 0 },
  model: "m1",
  llmCalls: 4,
  llmSkipped: 0,
  passed: 8,
  suspect: 1,
  failed: 1,
  revoked: [],
  siblingsRequeued: 0,
  siblingsRemaining: 0,
  missedSlots: 0,
  vaultFindings: [],
  createdAt: epochMs(1_700_000_000_000),
  startedAt: epochMs(1_700_000_001_000),
  finishedAt: epochMs(1_700_000_002_000),
  ...over,
});

export const checkWith = (over: Partial<AuditCheck> = {}): AuditCheck => ({
  id: "chk-1",
  rank: 0,
  claimId: "claim-1",
  outcome: "passed",
  autoCommitted: false,
  countedCorrect: false,
  combination: { extractor: "m1", promptVersion: "p1", itemType: "claim" },
  changesetId: null,
  checks: [{ check: "quote_relocation", status: "pass", cause: null, detail: "found", model: null, tokens: null }],
  ...over,
});

export const usageRowWith = (over: Partial<AgentUsageRow> = {}): AgentUsageRow => ({
  agent: "extraction",
  model: "m1",
  vault: vaultId("v"),
  day: "2026-11-03",
  count: 1,
  tokens: { input: 100, output: 10, thinking: 0 },
  costUsd: null,
  ...over,
});

export const DEFAULT_SCHEDULE: AuditSchedule = { enabled: false, intervalMs: 86_400_000, sampleSize: 50, runTokenBudget: 200_000, dayTokenBudget: 500_000, ttlDays: {}, nextRunAt: null, totalMissedSlots: 0 };

/** The audit routes in memory. A started run is queued and gets the next id. */
export const fakeAuditApi = (seed: AuditSeed): AuditApi & { readonly started: AuditRun[] } => {
  const runs: AuditRun[] = [...(seed.runs ?? [])];
  const started: AuditRun[] = [];
  let schedule = seed.schedule ?? DEFAULT_SCHEDULE;
  const size = seed.checkPageSize ?? 200;
  const forbidden = failure(403, "forbidden", "needs admin scope");
  return {
    started,
    listAuditRuns: () => Promise.resolve(ok({ items: [...runs], nextCursor: null })),
    getAuditRun: (_vault, id) => {
      const run = runs.find((r) => r.id === id);
      return Promise.resolve(run === undefined ? failed(failure(404, "not_found", "no such run")) : ok({ run, sample: (seed.checks?.[id] ?? []).map((c) => c.claimId) }));
    },
    listAuditChecks: (_vault, run, findingsOnly, cursor) => {
      const all = (seed.checks?.[run] ?? []).filter((c) => !findingsOnly || c.outcome !== "passed");
      const from = cursor === null ? 0 : Number(cursor);
      return Promise.resolve(ok({ items: all.slice(from, from + size), nextCursor: from + size < all.length ? String(from + size) : null }));
    },
    triggerAudit: (_vault, trigger) => {
      if (seed.readOnly === true) return Promise.resolve(failed(forbidden));
      const run = runWith({ id: `run-${String(runs.length + 1)}`, state: "queued", seed: trigger.seed ?? "7", sampleSize: trigger.sampleSize ?? schedule.sampleSize, runTokenBudget: trigger.runTokenBudget ?? schedule.runTokenBudget, finishedAt: null, startedAt: null });
      runs.push(run);
      started.push(run);
      return Promise.resolve(ok(run));
    },
    getAuditSchedule: () => Promise.resolve(ok(schedule)),
    putAuditSchedule: (_vault, input) => {
      if (seed.readOnly === true) return Promise.resolve(failed(forbidden));
      if (input.intervalMs < 60_000) return Promise.resolve(failed(failure(400, "invalid", "the interval must be at least a minute")));
      schedule = { ...input, nextRunAt: input.enabled ? epochMs(1_700_000_000_000 + input.intervalMs) : null, totalMissedSlots: schedule.totalMissedSlots };
      return Promise.resolve(ok(schedule));
    },
    getAuditBudget: () => Promise.resolve(ok({ runTokenBudget: schedule.runTokenBudget, dayTokenBudget: schedule.dayTokenBudget, daySpent: 0, dayRemaining: schedule.dayTokenBudget })),
    getAuditRecall: () => (seed.recall === undefined ? Promise.resolve(failed(failure(404, "not_found", "no report"))) : Promise.resolve(ok(seed.recall))),
    getAgentUsage: () => Promise.resolve(seed.usage === undefined ? failed(forbidden) : ok(seed.usage)),
  };
};
