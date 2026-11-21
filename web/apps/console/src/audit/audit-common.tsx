import { auditRunStatus } from "@memex/core";
import type { AuditTone } from "@memex/core";
import type { AuditRun } from "@memex/types";
import { Badge } from "@memex/ui";
import { useQueryClient } from "@tanstack/react-query";
import { useCallback } from "react";
import { QUERY_KEYS } from "../query";

/** How often an open run is read again, so it moves on screen without a reload. A sample is at most 1,000 claims, so this is cheap. */
export const OPEN_RUN_POLL_MS = 1000;
/** With nothing open, a scheduled run still appears within this long. */
export const IDLE_RUNS_POLL_MS = 10_000;

export const ToneBadge = ({ tone, children, testId }: { readonly tone: AuditTone; readonly children: string; readonly testId?: string }) => (
  <span data-testid={testId}><Badge tone={tone}>{children}</Badge></span>
);

export const RunStateBadge = ({ run }: { readonly run: AuditRun }) => {
  const s = auditRunStatus(run);
  return <ToneBadge tone={s.tone} testId="run-state">{s.label}</ToneBadge>;
};

export const useRefreshAudit = (vault: string) => {
  const queryClient = useQueryClient();
  return useCallback(() => queryClient.invalidateQueries({ queryKey: QUERY_KEYS.audit(vault) }), [queryClient, vault]);
};

/** A labelled figure, for a run's summary. */
export const Fact = ({ label, children, testId }: { readonly label: string; readonly children: React.ReactNode; readonly testId?: string }) => (
  <div className="flex flex-col">
    <dt className="text-xs text-text-muted">{label}</dt>
    <dd className="tabular-nums" data-testid={testId}>{children}</dd>
  </div>
);
