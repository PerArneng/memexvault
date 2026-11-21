import { describe, expect, it } from "vitest";
import type { components } from "./generated/schema";
import { agentUsageFromWire, auditCheckFromWire, auditRunFromWire, auditScheduleFromWire, auditScheduleToWire, auditTriggerToWire, recallFromWire } from "./translate-audit";

type Schemas = components["schemas"];

const tokens = { input_tokens: 10, output_tokens: 2, thinking_tokens: 1 };
const run: Schemas["AuditRunDto"] = {
  id: "r", seed: "18446744073709551615", trigger: "on_demand", requested_by: "pat", state: "finished", stop: "run_budget", stop_detail: "ran out",
  sample_size: 5, population: 9, sampled: 5, run_token_budget: 100, tokens, model: null, llm_calls: 2, llm_skipped: 3, passed: 4, suspect: 0, failed: 1,
  revoked: ["combo"], siblings_requeued: 1, siblings_remaining: 0, missed_slots: 0, vault_findings: [{ check: "index_reconciliation", status: "inconclusive", detail: "<b>x</b>" }],
  created_at_ms: 1, started_at_ms: null, finished_at_ms: 3,
};

describe("audit translation", () => {
  it("keeps a seed as text and the run's every figure", () => {
    expect(auditRunFromWire(run)).toMatchObject({ seed: "18446744073709551615", stop: "run_budget", tokens: { input: 10, output: 2, thinking: 1 }, model: null, startedAt: null, finishedAt: 3, vaultFindings: [{ detail: "<b>x</b>" }] });
  });
  it("reads a person's claim, with no combination, and a check's tokens", () => {
    const c = auditCheckFromWire({ id: "c", rank: 0, run_id: "r", claim_id: "a", outcome: "failed", auto_committed: false, counted_correct: false, combination: null, changeset_id: "cs", finished_at_ms: 1, weight_micro: 1, checks: [{ check: "entailment", status: "fail", cause: "not_entailed", detail: "d", model: "m", prompt_hash: "h", tokens }] });
    expect(c).toMatchObject({ combination: null, changesetId: "cs", checks: [{ cause: "not_entailed", tokens: { input: 10 } }] });
  });
  it("sends only what's set when starting a run", () => {
    expect(auditTriggerToWire({ seed: null, sampleSize: null, runTokenBudget: null })).toEqual({});
    expect(auditTriggerToWire({ seed: "1", sampleSize: 2, runTokenBudget: 0 })).toEqual({ seed: "1", sample_size: 2, run_token_budget: 0 });
  });
  it("round-trips a schedule", () => {
    const wire: Schemas["AuditScheduleDto"] = { enabled: true, interval_ms: 60_000, sample_size: 3, run_token_budget: 4, day_token_budget: 5, ttl_days: { p: 2 }, next_run_at_ms: null, total_missed_slots: 7 };
    const s = auditScheduleFromWire(wire);
    expect(s).toMatchObject({ nextRunAt: null, totalMissedSlots: 7, ttlDays: { p: 2 } });
    expect(auditScheduleToWire(s)).toEqual({ enabled: true, interval_ms: 60_000, sample_size: 3, run_token_budget: 4, day_token_budget: 5, ttl_days: { p: 2 } });
  });
  it("keeps the two recall groups apart and an unjudged target null", () => {
    const g = { planted: 1, audited: 1, caught: 0, recall: null };
    const r = recallFromWire({ kinds: [{ ...g, kind: "wrong_value", needs_llm: true }], deterministic: g, llm_only: { ...g, recall: 0 }, overall_planted: 1, overall_caught: 0, overall_recall: null, clean_audited: 0, false_positives_deterministic: 0, false_positives_llm: 0, llm_evidence: "none", llm_models: [], target: 0.8, target_met: null, too_small: true });
    expect(r).toMatchObject({ tooSmall: true, targetMet: null, deterministic: { recall: null }, llmOnly: { recall: 0 }, kinds: [{ needsLlm: true }] });
  });
  it("keeps a missing cost missing", () => {
    const u = agentUsageFromWire({ rows: [{ agent: "audit", model: "m", vault_id: "v", day: "2026-11-01", derivations: 2, input_tokens: 1, output_tokens: 2, thinking_tokens: 3, cost_usd: null }], priced_cost_usd: 0, unpriced_tokens: 6 });
    expect(u.rows[0]).toMatchObject({ count: 2, costUsd: null, vault: "v" });
  });
});
