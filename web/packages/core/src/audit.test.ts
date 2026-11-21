import { vaultId } from "@memex/types";
import type { Identity, RecallReport } from "@memex/types";
import { describe, expect, it } from "vitest";
import {
  auditAccess,
  auditAccessText,
  auditRunStatus,
  budgetUsedText,
  checkName,
  formatCount,
  intervalText,
  noReviewItemText,
  parseSchedule,
  parseTrigger,
  recallText,
  recallWarnings,
  runIsOpen,
  scheduleForm,
  seedProblem,
  targetText,
} from "./audit";
import { checkWith, DEFAULT_SCHEDULE, runWith } from "./testing-audit";

const vault = vaultId("v");
const identity = (scope: "read" | "propose" | "admin" | null, serverAdmin = false): Identity => ({
  principal: { id: "p", name: "p", serverAdmin, machine: false },
  tokenId: "t",
  grants: scope === null ? [] : [{ vault, scope }],
});

const form = { enabled: true, intervalMinutes: "60", sampleSize: "50", runTokenBudget: "1000", dayTokenBudget: "5000", ttl: "" };

const recall = (over: Partial<RecallReport> = {}): RecallReport => ({
  kinds: [],
  deterministic: { planted: 80, audited: 80, caught: 80, recall: 1 },
  llmOnly: { planted: 32, audited: 32, caught: 0, recall: 0 },
  overallPlanted: 112,
  overallCaught: 80,
  overallRecall: 80 / 112,
  cleanAudited: 60,
  falsePositivesDeterministic: 0,
  falsePositivesLlm: 0,
  llmEvidence: "model",
  llmModels: ["m"],
  target: 0.8,
  targetMet: false,
  tooSmall: false,
  ...over,
});

describe("audit access", () => {
  it("lets admins start runs and anyone with read look", () => {
    expect(auditAccess(identity("admin"), vault)).toEqual({ kind: "allowed" });
    expect(auditAccess(identity(null, true), vault)).toEqual({ kind: "allowed" });
    expect(auditAccess(identity("read"), vault)).toEqual({ kind: "needs_admin", have: "read" });
    expect(auditAccess(identity("propose"), vault)).toEqual({ kind: "needs_admin", have: "propose" });
    expect(auditAccessText(auditAccess(identity("read"), vault))).toContain("your token has read");
    expect(auditAccessText(auditAccess(identity(null), vault))).toContain("none here");
    expect(auditAccessText({ kind: "allowed" })).toBeNull();
  });
});

describe("run status", () => {
  it("says a budget stop is a stop and not a failure", () => {
    for (const stop of ["run_budget", "day_budget"]) {
      const s = auditRunStatus(runWith({ stop }));
      expect(s.label).toBe("Stopped by budget");
      expect(s.tone).toBe("warning");
      expect(s.explain).toContain("isn't a failure");
    }
  });
  it("names every state, and only a job that gave up is failed", () => {
    expect(auditRunStatus(runWith({ state: "queued" })).label).toBe("Queued");
    expect(auditRunStatus(runWith({ state: "running" })).label).toBe("Running");
    expect(auditRunStatus(runWith({ state: "finished" })).label).toBe("Finished");
    expect(auditRunStatus(runWith({ state: "failed", stop: "run_budget" })).label).toBe("Failed");
    expect(auditRunStatus(runWith({ stop: "llm_unavailable" })).label).toBe("LLM checks stopped");
    expect(auditRunStatus(runWith({ stop: "llm_refused" })).explain).toContain("refused");
    expect(auditRunStatus(runWith({ stop: "something_new" })).explain).toContain("something_new");
    expect(auditRunStatus(runWith({ state: "odd" })).label).toBe("odd");
  });
  it("keeps reading only open runs", () => {
    expect(runIsOpen(runWith({ state: "queued" }))).toBe(true);
    expect(runIsOpen(runWith({ state: "running" }))).toBe(true);
    expect(runIsOpen(runWith({ state: "finished" }))).toBe(false);
    expect(runIsOpen(runWith({ state: "failed" }))).toBe(false);
  });
  it("says how much budget went", () => {
    expect(budgetUsedText(runWith({ tokens: { input: 900, output: 100, thinking: 0 }, runTokenBudget: 4000 }))).toBe("1,000 of 4,000 tokens (25%)");
    expect(budgetUsedText(runWith({ tokens: { input: 5000, output: 0, thinking: 0 }, runTokenBudget: 4000 }))).toBe("5,000 of 4,000 tokens (100%)");
    expect(budgetUsedText(runWith({ tokens: { input: 0, output: 0, thinking: 0 }, runTokenBudget: 0 }))).toContain("no budget");
  });
  it("formats counts without the locale", () => {
    expect(formatCount(0)).toBe("0");
    expect(formatCount(999)).toBe("999");
    expect(formatCount(1234567)).toBe("1,234,567");
    expect(formatCount(-1500)).toBe("-1,500");
  });
});

describe("findings", () => {
  it("names checks, and says why a finding has no review item", () => {
    expect(checkName("quote_relocation")).toBe("Quote is where it says");
    expect(checkName("blind_reextraction")).toContain("re-extraction");
    expect(checkName("new_check")).toBe("new check");
    expect(noReviewItemText(checkWith({ outcome: "failed", changesetId: null }))).toContain("No review item");
    expect(noReviewItemText(checkWith({ outcome: "failed", changesetId: "cs" }))).toBeNull();
    expect(noReviewItemText(checkWith({ outcome: "passed" }))).toBeNull();
  });
});

describe("starting a run", () => {
  it("leaves empty fields to the schedule", () => {
    expect(parseTrigger({ seed: "", sampleSize: "", runTokenBudget: "" })).toEqual({ ok: true, value: { seed: null, sampleSize: null, runTokenBudget: null } });
    expect(parseTrigger({ seed: " 42 ", sampleSize: "10", runTokenBudget: "1,000" })).toEqual({ ok: true, value: { seed: "42", sampleSize: 10, runTokenBudget: 1000 } });
  });
  it("keeps a seed past what a JSON number holds as text", () => {
    expect(parseTrigger({ seed: "18446744073709551615", sampleSize: "", runTokenBudget: "" })).toMatchObject({ ok: true, value: { seed: "18446744073709551615" } });
    expect(seedProblem("18446744073709551616")).toContain("at most");
  });
  it("refuses what the server would", () => {
    const bad = parseTrigger({ seed: "-1", sampleSize: "0", runTokenBudget: "lots" });
    expect(bad.ok).toBe(false);
    if (!bad.ok) expect(bad.problems).toHaveLength(3);
    expect(parseTrigger({ seed: "1.5", sampleSize: "1001", runTokenBudget: "" }).ok).toBe(false);
  });
});

describe("the schedule", () => {
  it("round-trips the server's schedule through the form", () => {
    const s = { ...DEFAULT_SCHEDULE, enabled: true, ttlDays: { "built in": 30, price: 7 } };
    expect(parseSchedule(scheduleForm(s))).toEqual({ ok: true, value: { enabled: true, intervalMs: s.intervalMs, sampleSize: 50, runTokenBudget: 200_000, dayTokenBudget: 500_000, ttlDays: { "built in": 30, price: 7 } } });
  });
  it("names every problem at once, as the server's rules have them", () => {
    const bad = parseSchedule({ ...form, intervalMinutes: "0", sampleSize: "2000", runTokenBudget: "", dayTokenBudget: "x", ttl: "price = 0\nno equals\n = 3" });
    expect(bad.ok).toBe(false);
    if (!bad.ok) {
      expect(bad.problems[0]).toContain("at least 1");
      expect(bad.problems.some((p) => p.includes("1 to 1000"))).toBe(true);
      expect(bad.problems.filter((p) => p.startsWith("Line"))).toHaveLength(3);
      expect(bad.problems).toHaveLength(7);
    }
  });
  it("refuses a predicate given twice", () => {
    const bad = parseSchedule({ ...form, ttl: "price = 3\nprice = 4" });
    expect(bad.ok ? [] : bad.problems).toEqual(['"price" has more than one TTL.']);
  });
  it("says an interval in the largest whole unit", () => {
    expect(intervalText(86_400_000)).toBe("every 1 day");
    expect(intervalText(2 * 86_400_000)).toBe("every 2 days");
    expect(intervalText(3_600_000 * 5)).toBe("every 5 hours");
    expect(intervalText(90 * 60_000)).toBe("every 90 minutes");
    expect(intervalText(60_000)).toBe("every 1 minute");
  });
});

describe("recall", () => {
  it("warns when too few errors were planted", () => {
    const w = recallWarnings(recall({ tooSmall: true, overallPlanted: 14 }));
    expect(w[0]?.title).toBe("Too few planted errors to trust this figure.");
    expect(w[0]?.detail).toContain("14 planted");
  });
  it("says scripted answers prove nothing about a model, and nothing planted means nothing to read", () => {
    expect(recallWarnings(recall({ llmEvidence: "scripted" })).map((w) => w.title)).toEqual(["The LLM figures come from scripted answers."]);
    expect(recallWarnings(recall({ llmEvidence: "none" })).map((w) => w.title)).toEqual(["No LLM has answered yet."]);
    expect(recallWarnings(recall({ overallPlanted: 0, tooSmall: true }))).toHaveLength(1);
    expect(recallWarnings(recall())).toEqual([]);
  });
  it("says what's measured and whether the target is judged", () => {
    expect(recallText(null)).toBe("Not measured");
    expect(recallText(0.714285)).toBe("71.4%");
    expect(targetText(recall({ targetMet: null }))).toContain("isn't judged yet");
    expect(targetText(recall({ targetMet: true }))).toBe("Meets the 80% target.");
    expect(targetText(recall({ targetMet: false }))).toBe("Below the 80% target.");
  });
});
