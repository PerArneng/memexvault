import type { VaultId } from "@memex/types";
import { isMonth } from "./audit-usage";
import type { Month } from "./audit-usage";
import { sectionPath } from "./navigation";

/** Where an Audit URL points. The `-` keeps a tab's name from ever meaning a run, whatever id one has. */
export type AuditRoute =
  | { readonly kind: "runs" }
  | { readonly kind: "run"; readonly id: string; readonly findingsOnly: boolean }
  | { readonly kind: "schedule" }
  | { readonly kind: "recall" }
  /** Null month means this month. */
  | { readonly kind: "agents"; readonly month: Month | null }
  | { readonly kind: "unknown" };

/** `rest` is the path after `audit`, already decoded. A URL anyone typed lands on `unknown` or a default rather than throwing. */
export const parseAuditRoute = (rest: readonly string[], query: (name: string) => string | null): AuditRoute => {
  const [first, second, third] = rest;
  if (first === undefined) return { kind: "runs" };
  if (first !== "-") return second === undefined ? { kind: "run", id: first, findingsOnly: query("findings") === "true" || query("findings") === "1" } : { kind: "unknown" };
  if (third !== undefined) return { kind: "unknown" };
  if (second === "schedule") return { kind: "schedule" };
  if (second === "recall") return { kind: "recall" };
  if (second === "agents") {
    const month = query("month");
    // A month that can't be read shows this month, rather than an error.
    return { kind: "agents", month: month !== null && isMonth(month) ? month : null };
  }
  return { kind: "unknown" };
};

export const auditPath = (vault: VaultId, route: Exclude<AuditRoute, { readonly kind: "unknown" }>): string => {
  const root = sectionPath(vault, "audit");
  switch (route.kind) {
    case "runs":
      return root;
    case "run":
      return `${root}/${encodeURIComponent(route.id)}${route.findingsOnly ? "?findings=true" : ""}`;
    case "schedule":
      return `${root}/-/schedule`;
    case "recall":
      return `${root}/-/recall`;
    case "agents":
      return route.month === null ? `${root}/-/agents` : `${root}/-/agents?month=${route.month}`;
  }
};
