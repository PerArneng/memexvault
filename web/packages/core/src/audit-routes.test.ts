import { vaultId } from "@memex/types";
import { describe, expect, it } from "vitest";
import { auditPath, parseAuditRoute } from "./audit-routes";

const vault = vaultId("v");
const none = () => null;
const query = (values: Readonly<Record<string, string>>) => (name: string) => values[name] ?? null;

describe("audit routes", () => {
  it("reads every tab and a run", () => {
    expect(parseAuditRoute([], none)).toEqual({ kind: "runs" });
    expect(parseAuditRoute(["r1"], none)).toEqual({ kind: "run", id: "r1", findingsOnly: false });
    expect(parseAuditRoute(["r1"], query({ findings: "true" }))).toEqual({ kind: "run", id: "r1", findingsOnly: true });
    expect(parseAuditRoute(["-", "schedule"], none)).toEqual({ kind: "schedule" });
    expect(parseAuditRoute(["-", "recall"], none)).toEqual({ kind: "recall" });
    expect(parseAuditRoute(["-", "agents"], query({ month: "2026-11" }))).toEqual({ kind: "agents", month: "2026-11" });
  });
  it("shows this month for a month it can't read, and unknown for what no link makes", () => {
    expect(parseAuditRoute(["-", "agents"], query({ month: "2026-13" }))).toEqual({ kind: "agents", month: null });
    for (const rest of [["-"], ["-", "nope"], ["r", "x"], ["-", "schedule", "x"]]) expect(parseAuditRoute(rest, none)).toEqual({ kind: "unknown" });
  });
  it("makes the paths it reads", () => {
    expect(auditPath(vault, { kind: "runs" })).toBe("/vaults/v/audit");
    expect(auditPath(vault, { kind: "run", id: "a/b", findingsOnly: true })).toBe("/vaults/v/audit/a%2Fb?findings=true");
    expect(auditPath(vault, { kind: "schedule" })).toBe("/vaults/v/audit/-/schedule");
    expect(auditPath(vault, { kind: "recall" })).toBe("/vaults/v/audit/-/recall");
    expect(auditPath(vault, { kind: "agents", month: null })).toBe("/vaults/v/audit/-/agents");
    expect(auditPath(vault, { kind: "agents", month: "2026-01" })).toBe("/vaults/v/audit/-/agents?month=2026-01");
  });
});
