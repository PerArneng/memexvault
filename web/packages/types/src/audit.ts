import type { EpochMs, VaultId } from "./index";

/** Tokens one call, run or row spent. */
export interface TokenUse {
  readonly input: number;
  readonly output: number;
  readonly thinking: number;
}

/** Something about the vault as a whole, with no one claim to put in front of a person. Its detail is plain text. */
export interface VaultFinding {
  readonly check: string;
  readonly status: string;
  readonly detail: string;
}

/** One audit of a sample of a vault's claims. */
export interface AuditRun {
  readonly id: string;
  /** A decimal number, as text, as it can be too large for a JSON number. */
  readonly seed: string;
  /** `scheduled` or `on_demand`. */
  readonly trigger: string;
  readonly requestedBy: string;
  /** `queued`, `running`, `finished`, or `failed` if its job gave up. */
  readonly state: string;
  /** Why its LLM checks ended early: `run_budget`, `day_budget`, `llm_unavailable` or `llm_refused`. */
  readonly stop: string | null;
  readonly stopDetail: string | null;
  readonly sampleSize: number;
  readonly population: number;
  readonly sampled: number;
  readonly runTokenBudget: number;
  readonly tokens: TokenUse;
  readonly model: string | null;
  readonly llmCalls: number;
  readonly llmSkipped: number;
  readonly passed: number;
  readonly suspect: number;
  readonly failed: number;
  readonly revoked: readonly string[];
  readonly siblingsRequeued: number;
  readonly siblingsRemaining: number;
  readonly missedSlots: number;
  readonly vaultFindings: readonly VaultFinding[];
  readonly createdAt: EpochMs;
  readonly startedAt: EpochMs | null;
  readonly finishedAt: EpochMs | null;
}

export interface AuditRunDetail {
  readonly run: AuditRun;
  /** The claims it sampled, in rank order. */
  readonly sample: readonly string[];
}

export interface AuditCheckResult {
  readonly check: string;
  /** `pass`, `fail` or `inconclusive`. */
  readonly status: string;
  readonly cause: string | null;
  /** May hold a document's or a model's words, so it's shown as plain text. */
  readonly detail: string;
  readonly model: string | null;
  readonly tokens: TokenUse | null;
}

export interface AuditCombination {
  readonly extractor: string;
  readonly promptVersion: string;
  readonly itemType: string;
}

/** One sampled claim and what was found about it. A finding is one whose outcome isn't `passed`. */
export interface AuditCheck {
  readonly id: string;
  readonly rank: number;
  readonly claimId: string;
  /** `sampled`, `passed`, `suspect` or `failed`. */
  readonly outcome: string;
  readonly autoCommitted: boolean;
  readonly countedCorrect: boolean;
  /** Null for a person's claim. */
  readonly combination: AuditCombination | null;
  /** The review item a person decides this on. */
  readonly changesetId: string | null;
  readonly checks: readonly AuditCheckResult[];
}

export interface AuditSchedule {
  readonly enabled: boolean;
  readonly intervalMs: number;
  readonly sampleSize: number;
  readonly runTokenBudget: number;
  readonly dayTokenBudget: number;
  /** Days until claims of each predicate go stale. */
  readonly ttlDays: Readonly<Record<string, number>>;
  readonly nextRunAt: EpochMs | null;
  readonly totalMissedSlots: number;
}

export type AuditScheduleInput = Omit<AuditSchedule, "nextRunAt" | "totalMissedSlots">;

export interface AuditBudget {
  readonly runTokenBudget: number;
  readonly dayTokenBudget: number;
  readonly daySpent: number;
  readonly dayRemaining: number;
}

/** What to run. Anything null comes from the schedule. */
export interface AuditTrigger {
  readonly seed: string | null;
  readonly sampleSize: number | null;
  readonly runTokenBudget: number | null;
}

export interface GroupRecall {
  readonly planted: number;
  readonly audited: number;
  readonly caught: number;
  readonly recall: number | null;
}

export interface KindRecall extends GroupRecall {
  readonly kind: string;
  readonly needsLlm: boolean;
}

export interface RecallReport {
  readonly kinds: readonly KindRecall[];
  readonly deterministic: GroupRecall;
  readonly llmOnly: GroupRecall;
  readonly overallPlanted: number;
  readonly overallCaught: number;
  readonly overallRecall: number | null;
  readonly cleanAudited: number;
  readonly falsePositivesDeterministic: number;
  readonly falsePositivesLlm: number;
  /** `none`, `scripted` or `model`. */
  readonly llmEvidence: string;
  readonly llmModels: readonly string[];
  readonly target: number;
  readonly targetMet: boolean | null;
  /** Too few planted errors to trust the figure. */
  readonly tooSmall: boolean;
}

/** Tokens and cost for one agent, model, vault and UTC day. */
export interface AgentUsageRow {
  readonly agent: string;
  readonly model: string;
  readonly vault: VaultId;
  /** `YYYY-MM-DD`, UTC. */
  readonly day: string;
  readonly count: number;
  readonly tokens: TokenUse;
  /** US dollars, only where the server has a price for the model. */
  readonly costUsd: number | null;
}

export interface AgentUsageReport {
  readonly rows: readonly AgentUsageRow[];
  readonly pricedCostUsd: number;
  readonly unpricedTokens: number;
}
