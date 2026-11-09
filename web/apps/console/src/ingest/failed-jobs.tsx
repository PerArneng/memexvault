import { canChangeSource, ingestPath } from "@memex/core";
import type { ActivityWatch, BulkRetryOutcome } from "@memex/core";
import type { Identity, Job, VaultId } from "@memex/types";
import { Button, EmptyState } from "@memex/ui";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useMemo, useState } from "react";
import { useEngine } from "../engine-context";
import { QUERY_KEYS, unwrap } from "../query";
import { QueryView } from "../shell/query-view";
import { AppLink, Callout, failureText, formatTime } from "../wiki/wiki-common";

const lastError = (job: Job): string | null => job.attempts.findLast((a) => a.error !== null)?.error ?? null;

const List = ({ identity, vault, jobs, truncated, onDone }: { readonly identity: Identity; readonly vault: VaultId; readonly jobs: readonly Job[]; readonly truncated: boolean; readonly onDone: (outcome: BulkRetryOutcome) => void }) => {
  const engine = useEngine();
  const allowed = canChangeSource(identity, vault);
  const [picked, setPicked] = useState<ReadonlySet<string>>(new Set());
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  const chosen = useMemo(() => jobs.filter((j) => picked.has(j.id)).map((j) => j.id), [jobs, picked]);
  const all = chosen.length === jobs.length && jobs.length > 0;
  const toggle = (id: string) => {
    const next = new Set(picked);
    if (!next.delete(id)) next.add(id);
    setPicked(next);
  };
  const run = async () => {
    setBusy(true);
    const result = await engine.retryJobs(vault, chosen);
    setBusy(false);
    setConfirming(false);
    setPicked(new Set());
    onDone(result);
  };
  return (
    <div className="flex flex-col gap-3">
      {truncated && <p className="text-sm text-warning">There are more failed jobs than one browser list holds, so this shows the first of them.</p>}
      {allowed && (
        <div className="flex flex-wrap items-center gap-3">
          <label className="inline-flex items-center gap-1.5 text-sm">
            <input type="checkbox" checked={all} onChange={() => setPicked(all ? new Set() : new Set(jobs.map((j) => j.id)))} />
            Select all {jobs.length}
          </label>
          {confirming ? (
            <div role="group" aria-label="Confirm retry" className="flex flex-wrap items-center gap-2 rounded-md border border-border-strong p-2 text-sm">
              <span>Run {chosen.length} jobs again?</span>
              <Button variant="primary" disabled={busy} onClick={() => void run()}>Yes, retry {chosen.length}</Button>
              <Button variant="ghost" disabled={busy} onClick={() => setConfirming(false)}>Keep them as they are</Button>
            </div>
          ) : (
            <Button disabled={chosen.length === 0} onClick={() => setConfirming(true)}>Retry selected ({chosen.length})</Button>
          )}
        </div>
      )}
      <ul aria-label="Failed jobs" className="flex flex-col gap-2">
        {jobs.map((job) => {
          const error = lastError(job);
          return (
            <li key={job.id} data-failed-job={job.id} className="flex items-start gap-3 rounded-md border border-border p-2">
              {allowed && <input type="checkbox" aria-label={`Select job ${job.id}`} checked={picked.has(job.id)} onChange={() => toggle(job.id)} className="mt-1" />}
              <div className="flex min-w-0 flex-1 flex-col gap-1">
                <div className="flex flex-wrap items-baseline gap-2 text-sm">
                  <span>{job.kind}</span>
                  <span className="font-mono text-xs text-text-muted">{job.id}</span>
                  <span className="text-xs text-text-muted">{job.attempts.length} attempts{job.finishedAtMs !== null && `, stopped ${formatTime(job.finishedAtMs)}`}</span>
                  {job.sourceId !== null && <AppLink to={ingestPath(vault, { kind: "document", id: job.sourceId })}>Open the document</AppLink>}
                </div>
                {error !== null && <pre className="overflow-auto whitespace-pre-wrap rounded-sm bg-danger-surface p-1 font-mono text-xs text-danger">{error}</pre>}
              </div>
            </li>
          );
        })}
      </ul>
    </div>
  );
};

export const FailedJobs = ({ identity, vault, state, watch }: { readonly identity: Identity; readonly vault: VaultId; readonly state: "dead" | "cancelled"; readonly watch: ActivityWatch }) => {
  const engine = useEngine();
  const queryClient = useQueryClient();
  const query = useQuery({ queryKey: QUERY_KEYS.jobsInState(vault, state), queryFn: async () => unwrap(await engine.listJobsInState(vault, state)), staleTime: 0 });
  // Out here, as a retry that works empties the list it was made from.
  const [outcome, setOutcome] = useState<BulkRetryOutcome | null>(null);
  const onDone = (result: BulkRetryOutcome) => {
    setOutcome(result);
    watch.refresh();
    void queryClient.invalidateQueries({ queryKey: QUERY_KEYS.jobs(vault) });
  };
  return (
    <div className="flex flex-col gap-3">
      <AppLink to={ingestPath(vault, { kind: "board", stage: "", status: "", text: "" })}>Back to the board</AppLink>
      <h2 className="text-lg font-medium">Failed jobs</h2>
      <p className="text-sm text-text-muted">A dead job used up its attempts, and a cancelled one was stopped by someone. Neither carries on by itself.</p>
      <nav aria-label="Kind of failed job" className="flex gap-3 text-sm">
        <AppLink to={ingestPath(vault, { kind: "failed", state: "dead" })} aria-current={state === "dead" ? "page" : undefined}>Dead</AppLink>
        <AppLink to={ingestPath(vault, { kind: "failed", state: "cancelled" })} aria-current={state === "cancelled" ? "page" : undefined}>Cancelled</AppLink>
      </nav>
      {outcome !== null && (
        <Callout tone={outcome.failed.length === 0 ? "info" : "danger"} title={`Retried ${String(outcome.retried.length)} by ${identity.principal.name}.${outcome.failed.length === 0 ? "" : ` ${String(outcome.failed.length)} couldn't be retried.`}`}>
          {outcome.failed.map((f) => <div key={f.id} className="font-mono text-xs">{f.id}: {failureText(f.failure)}</div>)}
        </Callout>
      )}
      <QueryView query={query} label="the failed jobs" isEmpty={(d) => d.jobs.length === 0} empty={<EmptyState title={state === "dead" ? "No dead jobs. Nothing has run out of attempts." : "No cancelled jobs."} />}>
        {(data) => <List key={state} identity={identity} vault={vault} jobs={data.jobs} truncated={data.truncated} onDone={onDone} />}
      </QueryView>
    </div>
  );
};
