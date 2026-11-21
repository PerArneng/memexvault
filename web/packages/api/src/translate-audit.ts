import { epochMs, vaultId } from "@memex/types";
import type {
  AgentUsageReport,
  AuditBudget,
  AuditCheck,
  AuditRun,
  AuditRunDetail,
  AuditSchedule,
  AuditScheduleInput,
  AuditTrigger,
  GroupRecall,
  Page,
  RecallReport,
  TokenUse,
} from "@memex/types";
import type { components } from "./generated/schema";

type Schemas = components["schemas"];

const tokensFromWire = (t: Schemas["TokenUsageDto"]): TokenUse => ({ input: t.input_tokens, output: t.output_tokens, thinking: t.thinking_tokens });
const at = (ms: number | null | undefined) => (ms === null || ms === undefined ? null : epochMs(ms));

export const auditRunFromWire = (r: Schemas["AuditRunDto"]): AuditRun => ({
  id: r.id,
  seed: r.seed,
  trigger: r.trigger,
  requestedBy: r.requested_by,
  state: r.state,
  stop: r.stop ?? null,
  stopDetail: r.stop_detail ?? null,
  sampleSize: r.sample_size,
  population: r.population,
  sampled: r.sampled,
  runTokenBudget: r.run_token_budget,
  tokens: tokensFromWire(r.tokens),
  model: r.model ?? null,
  llmCalls: r.llm_calls,
  llmSkipped: r.llm_skipped,
  passed: r.passed,
  suspect: r.suspect,
  failed: r.failed,
  revoked: r.revoked,
  siblingsRequeued: r.siblings_requeued,
  siblingsRemaining: r.siblings_remaining,
  missedSlots: r.missed_slots,
  vaultFindings: r.vault_findings.map((f) => ({ check: f.check, status: f.status, detail: f.detail })),
  createdAt: epochMs(r.created_at_ms),
  startedAt: at(r.started_at_ms),
  finishedAt: at(r.finished_at_ms),
});

export const auditRunPageFromWire = (p: Schemas["AuditRunPage"]): Page<AuditRun> => ({ items: p.items.map(auditRunFromWire), nextCursor: p.next_cursor ?? null });

export const auditRunDetailFromWire = (d: Schemas["AuditRunDetailDto"]): AuditRunDetail => ({ run: auditRunFromWire(d.run), sample: d.sample });

export const auditCheckFromWire = (c: Schemas["AuditCheckDto"]): AuditCheck => ({
  id: c.id,
  rank: c.rank,
  claimId: c.claim_id,
  outcome: c.outcome,
  autoCommitted: c.auto_committed,
  countedCorrect: c.counted_correct,
  combination: c.combination === null || c.combination === undefined ? null : { extractor: c.combination.extractor, promptVersion: c.combination.prompt_version, itemType: c.combination.item_type },
  changesetId: c.changeset_id ?? null,
  checks: c.checks.map((r) => ({ check: r.check, status: r.status, cause: r.cause ?? null, detail: r.detail, model: r.model ?? null, tokens: r.tokens === null || r.tokens === undefined ? null : tokensFromWire(r.tokens) })),
});

export const auditCheckPageFromWire = (p: Schemas["AuditCheckPage"]): Page<AuditCheck> => ({ items: p.items.map(auditCheckFromWire), nextCursor: p.next_cursor ?? null });

export const auditScheduleFromWire = (s: Schemas["AuditScheduleDto"]): AuditSchedule => ({
  enabled: s.enabled,
  intervalMs: s.interval_ms,
  sampleSize: s.sample_size,
  runTokenBudget: s.run_token_budget,
  dayTokenBudget: s.day_token_budget,
  ttlDays: { ...s.ttl_days },
  nextRunAt: at(s.next_run_at_ms),
  totalMissedSlots: s.total_missed_slots,
});

export const auditScheduleToWire = (s: AuditScheduleInput): Schemas["AuditScheduleBody"] => ({
  enabled: s.enabled,
  interval_ms: s.intervalMs,
  sample_size: s.sampleSize,
  run_token_budget: s.runTokenBudget,
  day_token_budget: s.dayTokenBudget,
  ttl_days: { ...s.ttlDays },
});

export const auditTriggerToWire = (t: AuditTrigger): Schemas["TriggerAuditBody"] => ({
  ...(t.seed === null ? {} : { seed: t.seed }),
  ...(t.sampleSize === null ? {} : { sample_size: t.sampleSize }),
  ...(t.runTokenBudget === null ? {} : { run_token_budget: t.runTokenBudget }),
});

export const auditBudgetFromWire = (b: Schemas["AuditBudgetDto"]): AuditBudget => ({ runTokenBudget: b.run_token_budget, dayTokenBudget: b.day_token_budget, daySpent: b.day_spent, dayRemaining: b.day_remaining });

const groupFromWire = (g: Schemas["GroupRecallDto"]): GroupRecall => ({ planted: g.planted, audited: g.audited, caught: g.caught, recall: g.recall ?? null });

export const recallFromWire = (r: Schemas["RecallReportDto"]): RecallReport => ({
  kinds: r.kinds.map((k) => ({ ...groupFromWire(k), kind: k.kind, needsLlm: k.needs_llm })),
  deterministic: groupFromWire(r.deterministic),
  llmOnly: groupFromWire(r.llm_only),
  overallPlanted: r.overall_planted,
  overallCaught: r.overall_caught,
  overallRecall: r.overall_recall ?? null,
  cleanAudited: r.clean_audited,
  falsePositivesDeterministic: r.false_positives_deterministic,
  falsePositivesLlm: r.false_positives_llm,
  llmEvidence: r.llm_evidence,
  llmModels: r.llm_models,
  target: r.target,
  targetMet: r.target_met ?? null,
  tooSmall: r.too_small,
});

export const agentUsageFromWire = (u: Schemas["UsageDto"]): AgentUsageReport => ({
  rows: u.rows.map((r) => ({
    agent: r.agent,
    model: r.model,
    vault: vaultId(r.vault_id),
    day: r.day,
    count: r.derivations,
    tokens: { input: r.input_tokens, output: r.output_tokens, thinking: r.thinking_tokens },
    costUsd: r.cost_usd ?? null,
  })),
  pricedCostUsd: u.priced_cost_usd,
  unpricedTokens: u.unpriced_tokens,
});
