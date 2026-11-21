import type { AuditCheck, AuditRun, AuditScheduleInput, AuditTrigger, Identity, RecallReport, VaultId } from "@memex/types";
import { scopeIn } from "./navigation";

// ---- Who may do what

export type AuditAccess = { readonly kind: "allowed" } | { readonly kind: "needs_admin"; readonly have: string };

/** Starting a run and writing the schedule may spend tokens, so the server wants admin scope for both. Reading needs only read. */
export const auditAccess = (identity: Identity, vault: VaultId): AuditAccess => {
  const have = scopeIn(identity, vault);
  return have === "admin" ? { kind: "allowed" } : { kind: "needs_admin", have: have ?? "no" };
};

export const auditAccessText = (access: AuditAccess): string | null =>
  access.kind === "allowed" ? null : `Starting runs and changing the schedule need admin scope, and your token has ${access.have === "no" ? "none here" : access.have}. You can read everything here.`;

// ---- A run's state in words

export type AuditTone = "neutral" | "info" | "success" | "warning" | "danger";

export interface AuditRunStatus {
  readonly label: string;
  readonly tone: AuditTone;
  /** A sentence saying what it means, or null when the label says it all. */
  readonly explain: string | null;
}

const STOP_TEXT: Readonly<Record<string, string>> = {
  run_budget: "Its LLM checks stopped when the run's token budget ran out. Every sampled claim kept its deterministic checks, and running out isn't a failure.",
  day_budget: "Its LLM checks stopped when the vault's daily token budget ran out. Every sampled claim kept its deterministic checks, and running out isn't a failure.",
  llm_unavailable: "Its LLM checks stopped because the model's provider couldn't be reached. Every sampled claim kept its deterministic checks.",
  llm_refused: "Its LLM checks stopped because the model's provider refused a call. Every sampled claim kept its deterministic checks.",
};

/** Running out of budget is a stop, never a failure: only a run whose job gave up is failed. */
export const auditRunStatus = (run: AuditRun): AuditRunStatus => {
  switch (run.state) {
    case "queued":
      return { label: "Queued", tone: "neutral", explain: "Waiting for a worker to pick it up." };
    case "running":
      return { label: "Running", tone: "info", explain: null };
    case "failed":
      return { label: "Failed", tone: "danger", explain: "Its job gave up, so what it found may be incomplete." };
    case "finished": {
      if (run.stop === "run_budget" || run.stop === "day_budget") return { label: "Stopped by budget", tone: "warning", explain: STOP_TEXT[run.stop] ?? null };
      if (run.stop !== null) return { label: "LLM checks stopped", tone: "warning", explain: STOP_TEXT[run.stop] ?? `Its LLM checks stopped early (${run.stop}).` };
      return { label: "Finished", tone: "success", explain: null };
    }
    default:
      return { label: run.state, tone: "neutral", explain: null };
  }
};

/** Whether a run can still change, so the screen keeps reading it. */
export const runIsOpen = (run: AuditRun): boolean => run.state === "queued" || run.state === "running";

export const triggerText = (trigger: string): string => (trigger === "scheduled" ? "Scheduled" : trigger === "on_demand" ? "On demand" : trigger);

export const tokensTotal = (t: { readonly input: number; readonly output: number; readonly thinking: number }): number => t.input + t.output + t.thinking;

/** `12,345`, the same in every locale, as a pure package can't ask the browser. */
export const formatCount = (n: number): string => {
  const sign = n < 0 ? "-" : "";
  const digits = Math.round(Math.abs(n)).toString();
  return sign + digits.replace(/\B(?=(\d{3})+(?!\d))/g, ",");
};

/** How much of the run's budget went, in words. A budget of 0 means no LLM checks at all. */
export const budgetUsedText = (run: AuditRun): string => {
  const used = tokensTotal(run.tokens);
  if (run.runTokenBudget === 0) return `${formatCount(used)} tokens, with no budget for LLM checks`;
  const share = Math.min(100, Math.round((used / run.runTokenBudget) * 100));
  return `${formatCount(used)} of ${formatCount(run.runTokenBudget)} tokens (${String(share)}%)`;
};

// ---- Findings

export const OUTCOME_TEXT: Readonly<Record<string, { readonly label: string; readonly tone: AuditTone }>> = {
  passed: { label: "Passed", tone: "success" },
  suspect: { label: "Suspect", tone: "warning" },
  failed: { label: "Failed", tone: "danger" },
  sampled: { label: "Not checked yet", tone: "neutral" },
};

export const outcomeOf = (outcome: string): { readonly label: string; readonly tone: AuditTone } => OUTCOME_TEXT[outcome] ?? { label: outcome, tone: "neutral" };

const CHECK_TEXT: Readonly<Record<string, string>> = {
  quote_relocation: "Quote is where it says",
  link_rot: "Links still lead somewhere",
  staleness: "Not stale",
  index_reconciliation: "Indexes agree",
  entailment: "Quote supports the claim (LLM)",
  blind_reextraction: "Blind re-extraction agrees (LLM)",
};

export const checkName = (check: string): string => CHECK_TEXT[check] ?? check.replace(/_/g, " ");

export const CHECK_STATUS: Readonly<Record<string, { readonly label: string; readonly tone: AuditTone }>> = {
  pass: { label: "Pass", tone: "success" },
  fail: { label: "Fail", tone: "danger" },
  inconclusive: { label: "Inconclusive", tone: "neutral" },
};

export const checkStatusOf = (status: string): { readonly label: string; readonly tone: AuditTone } => CHECK_STATUS[status] ?? { label: status, tone: "neutral" };

export const isFinding = (check: AuditCheck): boolean => check.outcome === "suspect" || check.outcome === "failed";

/** Why a finding has no review item, which a reader would otherwise take for a missing link. */
export const noReviewItemText = (check: AuditCheck): string | null => {
  if (!isFinding(check) || check.changesetId !== null) return null;
  return "No review item: there's nothing to retire, such as a claim only an index has lost, or a person already kept this claim.";
};

// ---- Forms

/** The largest seed, as the server's is an unsigned 64-bit number. */
const MAX_SEED = 18446744073709551615n;
export const MAX_SAMPLE = 1000;
const MAX_TOKENS = Number.MAX_SAFE_INTEGER;

export type Parsed<T> = { readonly ok: true; readonly value: T } | { readonly ok: false; readonly problems: readonly string[] };

/** A whole number from what someone typed, or null with nothing typed. Commas and spaces between digits are allowed. */
const wholeNumber = (raw: string): number | null | "bad" => {
  const text = raw.trim().replace(/[,_\s]/g, "");
  if (text === "") return null;
  if (!/^\d+$/.test(text)) return "bad";
  const n = Number(text);
  return Number.isSafeInteger(n) ? n : "bad";
};

export const seedProblem = (raw: string): string | null => {
  const text = raw.trim();
  if (text === "") return null;
  if (!/^\d+$/.test(text)) return "A seed is a whole number, such as 42.";
  return BigInt(text) > MAX_SEED ? "A seed must be at most 18446744073709551615." : null;
};

export interface TriggerForm {
  readonly seed: string;
  readonly sampleSize: string;
  readonly runTokenBudget: string;
}

/** Empty fields come from the schedule. */
export const parseTrigger = (form: TriggerForm): Parsed<AuditTrigger> => {
  const problems: string[] = [];
  const seed = seedProblem(form.seed);
  if (seed !== null) problems.push(seed);
  const sample = wholeNumber(form.sampleSize);
  if (sample === "bad" || (sample !== null && (sample < 1 || sample > MAX_SAMPLE))) problems.push(`The sample size must be a whole number from 1 to ${String(MAX_SAMPLE)}.`);
  const budget = wholeNumber(form.runTokenBudget);
  if (budget === "bad" || (budget !== null && budget > MAX_TOKENS)) problems.push("The token budget must be a whole number, 0 or more.");
  if (problems.length > 0) return { ok: false, problems };
  return {
    ok: true,
    value: {
      seed: form.seed.trim() === "" ? null : form.seed.trim(),
      sampleSize: sample === "bad" ? null : sample,
      runTokenBudget: budget === "bad" ? null : budget,
    },
  };
};

export interface ScheduleForm {
  readonly enabled: boolean;
  /** In minutes, as hours or days would hide the server's one-minute floor. */
  readonly intervalMinutes: string;
  readonly sampleSize: string;
  readonly runTokenBudget: string;
  readonly dayTokenBudget: string;
  /** One `predicate = days` a line. */
  readonly ttl: string;
}

export const scheduleForm = (s: AuditScheduleInput): ScheduleForm => ({
  enabled: s.enabled,
  intervalMinutes: String(Math.round(s.intervalMs / 60_000)),
  sampleSize: String(s.sampleSize),
  runTokenBudget: String(s.runTokenBudget),
  dayTokenBudget: String(s.dayTokenBudget),
  ttl: Object.entries(s.ttlDays).map(([p, d]) => `${p} = ${String(d)}`).join("\n"),
});

/** The same rules as the server's `check_schedule`, so most mistakes are named before anything is sent. The server still decides. */
export const parseSchedule = (form: ScheduleForm): Parsed<AuditScheduleInput> => {
  const problems: string[] = [];
  const minutes = wholeNumber(form.intervalMinutes);
  if (minutes === null || minutes === "bad" || minutes < 1) problems.push("The interval must be a whole number of minutes, at least 1.");
  const sample = wholeNumber(form.sampleSize);
  if (sample === null || sample === "bad" || sample < 1 || sample > MAX_SAMPLE) problems.push(`The sample size must be a whole number from 1 to ${String(MAX_SAMPLE)}.`);
  const run = wholeNumber(form.runTokenBudget);
  if (run === null || run === "bad") problems.push("The run's token budget must be a whole number, 0 or more.");
  const day = wholeNumber(form.dayTokenBudget);
  if (day === null || day === "bad") problems.push("The daily token budget must be a whole number, 0 or more.");
  const ttlDays: Record<string, number> = {};
  form.ttl.split("\n").forEach((line, i) => {
    if (line.trim() === "") return;
    const at = line.lastIndexOf("=");
    const predicate = at < 0 ? "" : line.slice(0, at).trim();
    const days = at < 0 ? "bad" : wholeNumber(line.slice(at + 1));
    if (predicate === "" || days === null || days === "bad" || days < 1) problems.push(`Line ${String(i + 1)} of the TTLs must be a predicate, "=", and a whole number of days, at least 1.`);
    else if (predicate in ttlDays) problems.push(`"${predicate}" has more than one TTL.`);
    else ttlDays[predicate] = days;
  });
  if (Object.keys(ttlDays).length > 1000) problems.push("At most 1000 predicates can have a TTL.");
  if (problems.length > 0 || minutes === null || minutes === "bad" || sample === null || sample === "bad" || run === null || run === "bad" || day === null || day === "bad") return { ok: false, problems };
  return { ok: true, value: { enabled: form.enabled, intervalMs: minutes * 60_000, sampleSize: sample, runTokenBudget: run, dayTokenBudget: day, ttlDays } };
};

/** `every 1 day`, `every 90 minutes`. */
export const intervalText = (ms: number): string => {
  const minutes = Math.round(ms / 60_000);
  const plural = (n: number, unit: string) => `${String(n)} ${unit}${n === 1 ? "" : "s"}`;
  if (minutes % 1440 === 0) return `every ${plural(minutes / 1440, "day")}`;
  if (minutes % 60 === 0) return `every ${plural(minutes / 60, "hour")}`;
  return `every ${plural(minutes, "minute")}`;
};

// ---- Recall

export interface RecallWarning {
  readonly title: string;
  readonly detail: string;
}

/** What has to be said before anyone reads a recall figure. Each is shown, not merged, as each is a different reason to doubt it. */
export const recallWarnings = (r: RecallReport): readonly RecallWarning[] => {
  const out: RecallWarning[] = [];
  if (r.overallPlanted === 0) return [{ title: "Nothing has been planted in this vault.", detail: "Recall is measured in a test vault, by planting errors and auditing it." }];
  if (r.tooSmall) out.push({ title: "Too few planted errors to trust this figure.", detail: `${formatCount(r.overallPlanted)} planted errors is under the 100 a figure needs. A run or two either way would move it a lot.` });
  if (r.llmEvidence === "scripted") out.push({ title: "The LLM figures come from scripted answers.", detail: "They show the checks work, and say nothing about how well a real model catches errors." });
  if (r.llmEvidence === "none" && r.llmOnly.planted > 0) out.push({ title: "No LLM has answered yet.", detail: "Kinds only an LLM can catch show 0 until runs with a model have checked them." });
  return out;
};

export const recallText = (recall: number | null): string => (recall === null ? "Not measured" : `${(recall * 100).toFixed(1)}%`);

/** The target is judged only on a real model's answers with every kind planted, so null says why. */
export const targetText = (r: RecallReport): string => {
  const target = `${String(Math.round(r.target * 100))}%`;
  if (r.targetMet === null) return `The ${target} target isn't judged yet: that needs every kind planted and a real model's answers.`;
  return r.targetMet ? `Meets the ${target} target.` : `Below the ${target} target.`;
};
