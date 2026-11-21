import { epochMs, vaultId } from "@memex/types";
import { describe, expect, it } from "vitest";
import { agentName, costText, isMonth, monthName, monthOf, monthRange, shiftMonth, totalsOf, usageView } from "./audit-usage";
import { usageRowWith } from "./testing-audit";

const v = vaultId("v");

describe("months", () => {
  it("reads a time as its UTC month", () => {
    expect(monthOf(epochMs(0))).toBe("1970-01");
    expect(monthOf(epochMs(951_782_400_000))).toBe("2000-02");
    expect(monthOf(epochMs(1_700_000_000_000))).toBe("2023-11");
    expect(monthOf(epochMs(1_701_388_799_999))).toBe("2023-11");
    expect(monthOf(epochMs(1_701_388_800_000))).toBe("2023-12");
    expect(monthOf(epochMs(-1))).toBe("1969-12");
  });
  it("gives each month its days, leap years included", () => {
    expect(monthRange("2024-02")).toMatchObject({ from: "2024-02-01", to: "2024-02-29" });
    expect(monthRange("2023-02").days).toHaveLength(28);
    expect(monthRange("1900-02").days).toHaveLength(28);
    expect(monthRange("2000-02").days).toHaveLength(29);
    expect(monthRange("2026-11")).toMatchObject({ from: "2026-11-01", to: "2026-11-30" });
    expect(monthRange("2026-12").to).toBe("2026-12-31");
  });
  it("steps across years and refuses what isn't a month", () => {
    expect(shiftMonth("2026-01", -1)).toBe("2025-12");
    expect(shiftMonth("2026-12", 1)).toBe("2027-01");
    expect(shiftMonth("2026-05", 0)).toBe("2026-05");
    expect(monthName("2026-11")).toBe("November 2026");
    for (const bad of ["2026-13", "2026-1", "26-11", "2026-00", "x"]) expect(isMonth(bad)).toBe(false);
    expect(isMonth("2026-09")).toBe(true);
  });
});

describe("usage", () => {
  const report = {
    rows: [
      usageRowWith({ agent: "extraction", model: "m1", day: "2026-11-03", count: 2, tokens: { input: 100, output: 10, thinking: 1 }, costUsd: 0.5 }),
      usageRowWith({ agent: "extraction", model: "m1", day: "2026-11-04", count: 1, tokens: { input: 50, output: 5, thinking: 0 }, costUsd: 0.25 }),
      usageRowWith({ agent: "audit", model: "m2", day: "2026-11-03", count: 3, tokens: { input: 1000, output: 0, thinking: 0 }, costUsd: null }),
      usageRowWith({ agent: "chat", model: "m1", day: "2026-10-31", costUsd: 9 }),
      usageRowWith({ vault: vaultId("other"), day: "2026-11-03", costUsd: 7 }),
    ],
    pricedCostUsd: 16.75,
    unpricedTokens: 1000,
  };
  it("keeps only this vault and month, and sums like the server", () => {
    const view = usageView(report, v, "2026-11");
    expect(view.rows).toHaveLength(3);
    expect(view.totals).toEqual({ count: 6, tokens: { input: 1150, output: 15, thinking: 1 }, costUsd: 0.75, unpricedTokens: 1000 });
    expect(view.groups.map((g) => [g.agent, g.model])).toEqual([["audit", "m2"], ["extraction", "m1"]]);
    expect(view.groups[0]?.totals.costUsd).toBeNull();
    expect(view.agents).toEqual(["audit", "extraction"]);
    expect(view.days).toHaveLength(30);
    expect(view.days[2]).toEqual({ day: "2026-11-03", byAgent: [1000, 111] });
    expect(view.days[0]?.byAgent).toEqual([0, 0]);
  });
  it("shows no cost at all when nothing has a price", () => {
    const view = usageView({ rows: report.rows.map((r) => ({ ...r, costUsd: null })), pricedCostUsd: 0, unpricedTokens: 0 }, v, "2026-11");
    expect(view.totals.costUsd).toBeNull();
    expect(costText(view.totals.costUsd)).toBeNull();
  });
  it("is empty for a month with nothing", () => {
    const view = usageView(report, v, "2025-01");
    expect(view.rows).toEqual([]);
    expect(view.totals).toEqual(totalsOf([]));
    expect(view.groups).toEqual([]);
    expect(view.days.every((d) => d.byAgent.length === 0)).toBe(true);
  });
  it("writes a cost so a small one doesn't read as nothing", () => {
    expect(costText(0)).toBe("$0.00");
    expect(costText(1.234)).toBe("$1.23");
    expect(costText(0.0004)).toBe("$0.0004");
    expect(agentName("audit")).toBe("Audit");
    expect(agentName("review")).toBe("review");
  });
});
