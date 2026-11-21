import type { AgentUsageReport, AgentUsageRow, EpochMs, TokenUse, VaultId } from "@memex/types";

/** A month as `YYYY-MM`, in UTC, which is how the server counts days. */
export type Month = string;

const pad = (n: number) => String(n).padStart(2, "0");

export const monthOf = (at: EpochMs): Month => {
  // Civil date from days, as `Date` is banned in pure packages. Howard Hinnant's civil-from-days.
  const days = Math.floor(at / 86_400_000);
  const z = days + 719_468;
  const era = Math.floor(z / 146_097);
  const doe = z - era * 146_097;
  const yoe = Math.floor((doe - Math.floor(doe / 1460) + Math.floor(doe / 36_524) - Math.floor(doe / 146_096)) / 365);
  const doy = doe - (365 * yoe + Math.floor(yoe / 4) - Math.floor(yoe / 100));
  const mp = Math.floor((5 * doy + 2) / 153);
  const m = mp < 10 ? mp + 3 : mp - 9;
  const y = yoe + era * 400 + (m <= 2 ? 1 : 0);
  return `${String(y)}-${pad(m)}`;
};

export const isMonth = (text: string): boolean => /^\d{4}-(0[1-9]|1[0-2])$/.test(text);

const daysIn = (year: number, month: number): number => {
  if (month === 2) return (year % 4 === 0 && year % 100 !== 0) || year % 400 === 0 ? 29 : 28;
  return [4, 6, 9, 11].includes(month) ? 30 : 31;
};

/** The first and last day of a month, inclusive, as the usage route takes them. */
export const monthRange = (month: Month): { readonly from: string; readonly to: string; readonly days: readonly string[] } => {
  const [y, m] = month.split("-").map(Number) as [number, number];
  const count = daysIn(y, m);
  const days = Array.from({ length: count }, (_, i) => `${month}-${pad(i + 1)}`);
  return { from: `${month}-01`, to: `${month}-${pad(count)}`, days };
};

export const shiftMonth = (month: Month, by: number): Month => {
  const [y, m] = month.split("-").map(Number) as [number, number];
  const index = y * 12 + (m - 1) + by;
  return `${String(Math.floor(index / 12))}-${pad((index % 12) + 1)}`;
};

const MONTH_NAMES = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
export const monthName = (month: Month): string => {
  const [y, m] = month.split("-").map(Number) as [number, number];
  return `${MONTH_NAMES[m - 1] ?? "?"} ${String(y)}`;
};

export interface UsageTotals {
  readonly count: number;
  readonly tokens: TokenUse;
  /** Over rows with a price. Null when no row has one, so nothing that looks like a cost appears. */
  readonly costUsd: number | null;
  /** Tokens in rows with no price, which the cost leaves out. */
  readonly unpricedTokens: number;
}

const ZERO: UsageTotals = { count: 0, tokens: { input: 0, output: 0, thinking: 0 }, costUsd: null, unpricedTokens: 0 };

const add = (t: UsageTotals, r: AgentUsageRow): UsageTotals => ({
  count: t.count + r.count,
  tokens: { input: t.tokens.input + r.tokens.input, output: t.tokens.output + r.tokens.output, thinking: t.tokens.thinking + r.tokens.thinking },
  costUsd: r.costUsd === null ? t.costUsd : (t.costUsd ?? 0) + r.costUsd,
  unpricedTokens: r.costUsd === null ? t.unpricedTokens + r.tokens.input + r.tokens.output + r.tokens.thinking : t.unpricedTokens,
});

export const totalsOf = (rows: readonly AgentUsageRow[]): UsageTotals => rows.reduce(add, ZERO);

export interface UsageGroup {
  readonly agent: string;
  readonly model: string;
  readonly totals: UsageTotals;
}

export interface UsageDay {
  readonly day: string;
  /** Tokens by agent, in the order of `agents`. */
  readonly byAgent: readonly number[];
}

export interface UsageView {
  readonly rows: readonly AgentUsageRow[];
  readonly totals: UsageTotals;
  /** By agent and model, largest first. */
  readonly groups: readonly UsageGroup[];
  readonly agents: readonly string[];
  /** Every day of the month, with nothing on days nothing was spent. */
  readonly days: readonly UsageDay[];
}

/** One vault's share of the report, for one month. The server's report covers every vault, so this keeps only the vault's rows. */
export const usageView = (report: AgentUsageReport, vault: VaultId, month: Month): UsageView => {
  const { days } = monthRange(month);
  const rows = report.rows.filter((r) => r.vault === vault && r.day.startsWith(`${month}-`)).toSorted((a, b) => a.day.localeCompare(b.day) || a.agent.localeCompare(b.agent) || a.model.localeCompare(b.model));
  const byKey = new Map<string, UsageGroup>();
  for (const r of rows) {
    const key = JSON.stringify([r.agent, r.model]);
    const g = byKey.get(key) ?? { agent: r.agent, model: r.model, totals: ZERO };
    byKey.set(key, { ...g, totals: add(g.totals, r) });
  }
  const groups = [...byKey.values()].toSorted((a, b) => total(b.totals) - total(a.totals) || a.agent.localeCompare(b.agent) || a.model.localeCompare(b.model));
  const agents = [...new Set(rows.map((r) => r.agent))].toSorted();
  return {
    rows,
    totals: totalsOf(rows),
    groups,
    agents,
    days: days.map((day) => ({ day, byAgent: agents.map((a) => rows.filter((r) => r.day === day && r.agent === a).reduce((n, r) => n + r.tokens.input + r.tokens.output + r.tokens.thinking, 0)) })),
  };
};

const total = (t: UsageTotals) => t.tokens.input + t.tokens.output + t.tokens.thinking;

/** Dollars to the cent, or to four places below a cent, so a small cost doesn't read as nothing. Null shows as no cost, never as $0. */
export const costText = (usd: number | null): string | null => {
  if (usd === null) return null;
  if (usd !== 0 && Math.abs(usd) < 0.01) return `$${usd.toFixed(4)}`;
  return `$${usd.toFixed(2)}`;
};

export const AGENT_NAMES: Readonly<Record<string, string>> = { extraction: "Extraction", chat: "Chat", audit: "Audit" };
export const agentName = (agent: string): string => AGENT_NAMES[agent] ?? agent;
