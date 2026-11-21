import { auditAccess, auditAccessText, auditPath, budgetUsedText, formatCount, parseTrigger, runIsOpen, triggerText } from "@memex/core";
import type { TriggerForm } from "@memex/core";
import type { Identity, VaultId } from "@memex/types";
import { Button, EmptyState, Input } from "@memex/ui";
import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { useEngine } from "../engine-context";
import { QUERY_KEYS, unwrap } from "../query";
import { QueryView } from "../shell/query-view";
import { AppLink, failureText, formatTime, useGo } from "../wiki/wiki-common";
import { IDLE_RUNS_POLL_MS, OPEN_RUN_POLL_MS, RunStateBadge, useRefreshAudit } from "./audit-common";

const EMPTY_FORM: TriggerForm = { seed: "", sampleSize: "", runTokenBudget: "" };

const StartRun = ({ vault }: { readonly vault: VaultId }) => {
  const engine = useEngine();
  const refresh = useRefreshAudit(vault);
  const go = useGo();
  const [form, setForm] = useState(EMPTY_FORM);
  const [problems, setProblems] = useState<readonly string[]>([]);
  const [busy, setBusy] = useState(false);
  const start = async () => {
    const parsed = parseTrigger(form);
    if (!parsed.ok) {
      setProblems(parsed.problems);
      return;
    }
    setProblems([]);
    setBusy(true);
    const result = await engine.triggerAudit(vault, parsed.value);
    setBusy(false);
    if (!result.ok) {
      setProblems([failureText(result.failure)]);
      return;
    }
    await refresh();
    go(auditPath(vault, { kind: "run", id: result.value.id, findingsOnly: false }));
  };
  const field = (key: keyof TriggerForm) => ({ value: form[key], onChange: (e: React.ChangeEvent<HTMLInputElement>) => setForm({ ...form, [key]: e.target.value }) });
  return (
    <form
      aria-label="Start a run"
      className="flex flex-col gap-2 rounded-lg border border-border p-3"
      onSubmit={(e) => {
        e.preventDefault();
        void start();
      }}
    >
      <h2 className="font-medium">Start a run</h2>
      <p className="text-sm text-text-muted">A run samples claims and checks them against their sources. The same seed on the same vault samples the same claims. Anything left empty comes from the schedule.</p>
      <div className="flex flex-wrap items-end gap-3">
        <Input label="Seed" id="audit-seed" inputMode="numeric" placeholder="random" {...field("seed")} />
        <Input label="Sample size" id="audit-sample" inputMode="numeric" placeholder="from the schedule" {...field("sampleSize")} />
        <Input label="Token budget" id="audit-budget" inputMode="numeric" placeholder="from the schedule" {...field("runTokenBudget")} />
        <Button type="submit" variant="primary" disabled={busy}>{busy ? "Starting" : "Start run"}</Button>
      </div>
      {problems.length > 0 && (
        <ul role="alert" className="text-sm text-danger" data-testid="start-problems">
          {problems.map((p) => <li key={p}>{p}</li>)}
        </ul>
      )}
    </form>
  );
};

export const RunsView = ({ identity, vault }: { readonly identity: Identity; readonly vault: VaultId }) => {
  const engine = useEngine();
  const access = auditAccess(identity, vault);
  const query = useQuery({
    queryKey: QUERY_KEYS.auditRuns(vault),
    queryFn: async () => unwrap(await engine.listAuditRuns(vault)),
    // Reads again, often while a run is open, so a run started anywhere, or by the schedule, shows up and moves here by itself.
    refetchInterval: (q) => (q.state.data?.some(runIsOpen) === true ? OPEN_RUN_POLL_MS : IDLE_RUNS_POLL_MS),
  });
  const note = auditAccessText(access);
  return (
    <div className="flex flex-col gap-3">
      {access.kind === "allowed" ? <StartRun vault={vault} /> : <p className="rounded-md border border-border p-3 text-sm text-text-muted" data-testid="read-only">{note}</p>}
      <QueryView query={query} label="audit runs" isEmpty={(runs) => runs.length === 0} empty={<EmptyState title="No audit has run in this vault yet.">Runs start on the schedule, which is off until someone turns it on, or from here.</EmptyState>}>
        {(runs) => (
          <table className="w-full text-sm" data-testid="runs-table">
            <thead className="text-left text-xs text-text-muted">
              <tr><th className="p-1">Started</th><th className="p-1">State</th><th className="p-1">Trigger</th><th className="p-1">Seed</th><th className="p-1">Sample</th><th className="p-1">Findings</th><th className="p-1">Tokens</th></tr>
            </thead>
            <tbody>
              {runs.map((r) => (
                <tr key={r.id} className="border-t border-border" data-run={r.id}>
                  <td className="p-1"><AppLink to={auditPath(vault, { kind: "run", id: r.id, findingsOnly: false })}>{formatTime(r.createdAt)}</AppLink></td>
                  <td className="p-1"><RunStateBadge run={r} /></td>
                  <td className="p-1">{triggerText(r.trigger)}</td>
                  <td className="p-1 font-mono">{r.seed}</td>
                  <td className="p-1 tabular-nums">{formatCount(r.sampled)} of {formatCount(r.population)}</td>
                  <td className="p-1 tabular-nums" data-testid="run-findings">{formatCount(r.suspect + r.failed)}</td>
                  <td className="p-1 tabular-nums">{budgetUsedText(r)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </QueryView>
    </div>
  );
};
