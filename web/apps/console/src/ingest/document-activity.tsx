import { canChangeSource, documentPath, durationText, ingestPath, pipelineSteps, progressText, stageLabel, statusExplanation } from "@memex/core";
import type { ActivityWatch, BoardDocument } from "@memex/core";
import type { Identity, Job, VaultId } from "@memex/types";
import { Badge, Button, EmptyState } from "@memex/ui";
import type { Tone } from "@memex/ui";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useState } from "react";
import { useEngine } from "../engine-context";
import { QUERY_KEYS, unwrap } from "../query";
import { QueryView } from "../shell/query-view";
import { AppLink, Callout, failureText, formatTime, LINK_BUTTON } from "../wiki/wiki-common";
import { ConnectionBadge, connectionDetail, ProgressBar, StatusOf, useActivityState } from "./ingest-common";

const STEP_TONE: Readonly<Record<string, string>> = { seen: "bg-success", passed: "bg-success", current: "bg-info", todo: "bg-text-faint" };
const JOB_TONE: Readonly<Record<string, Tone>> = { queued: "neutral", running: "info", done: "success", dead: "danger", cancelled: "warning" };
const JOB_KIND: Readonly<Record<string, string>> = { rendition: "Reading the document", transcription: "Transcribing", extract: "Extracting claims" };

const StageTimeline = ({ doc }: { readonly doc: BoardDocument }) => (
  <ol aria-label="Stages" className="flex flex-col gap-2 border-l border-border pl-4">
    {pipelineSteps(doc).map((step) => (
      <li key={step.stage} data-timeline-stage={step.stage} data-step-state={step.state} aria-current={step.state === "current" ? "step" : undefined} className="relative flex items-baseline justify-between gap-4">
        <span aria-hidden="true" className={`absolute top-1.5 -left-[1.28rem] size-2 rounded-full ${step.state === "current" && doc.status === "failed" ? "bg-danger" : (STEP_TONE[step.state] ?? "bg-text-faint")}`} />
        <span className={`text-sm ${step.state === "todo" ? "text-text-muted" : "text-text"}`}>
          {stageLabel(step.stage)}
          {step.state === "passed" && <span className="text-xs text-text-muted"> · finished too fast to see</span>}
          {step.state === "todo" && <span className="sr-only"> · not reached yet</span>}
        </span>
        {step.atMs !== null && <time className="font-mono text-xs text-text-muted tabular-nums">{formatTime(step.atMs)}</time>}
      </li>
    ))}
  </ol>
);

const JobCard = ({ vault, job, identity, onChanged }: { readonly vault: VaultId; readonly job: Job; readonly identity: Identity; readonly onChanged: () => void }) => {
  const engine = useEngine();
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<{ readonly tone: "info" | "danger"; readonly text: string } | null>(null);
  const allowed = canChangeSource(identity, vault);
  const canRetry = job.state === "dead" || job.state === "cancelled";
  const canCancel = job.state === "queued" || job.state === "running";
  const who = identity.principal.name;

  const act = async (action: "retry" | "cancel") => {
    setBusy(true);
    const outcome = action === "retry" ? await engine.retryJob(vault, job.id) : await engine.cancelJob(vault, job.id);
    setBusy(false);
    setConfirming(false);
    setResult(outcome.ok ? { tone: "info", text: action === "retry" ? `Retried by ${who}.` : `Cancelled by ${who}.` } : { tone: "danger", text: failureText(outcome.failure) });
    onChanged();
  };

  return (
    <li data-job={job.id} data-job-state={job.state} className="flex flex-col gap-2 rounded-md border border-border p-3">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <span className="text-sm">{JOB_KIND[job.kind] ?? job.kind} <span className="font-mono text-xs text-text-muted">{job.id}</span></span>
        <Badge tone={JOB_TONE[job.state] ?? "neutral"}>{job.state}</Badge>
      </div>
      <p className="text-xs text-text-muted">
        {job.attempts.length} of {job.maxAttempts} attempts. Created {formatTime(job.createdAtMs)}{job.finishedAtMs !== null && `, finished ${formatTime(job.finishedAtMs)}`}.
      </p>
      {job.attempts.length > 0 && (
        <ol aria-label="Attempts" className="flex flex-col gap-1">
          {job.attempts.map((attempt, i) => (
            <li key={`${String(attempt.startedAtMs)}-${String(i)}`} data-attempt={i + 1} className="text-xs">
              <span className="text-text-muted">Attempt {i + 1}, {formatTime(attempt.startedAtMs)}{attempt.finishedAtMs !== null && `, took ${durationText(attempt.finishedAtMs - attempt.startedAtMs)}`}</span>
              {attempt.error !== null && <pre className="mt-1 overflow-auto whitespace-pre-wrap rounded-sm bg-danger-surface p-1 font-mono text-danger">{attempt.error}</pre>}
            </li>
          ))}
        </ol>
      )}
      {result !== null && <Callout tone={result.tone} title={result.text} />}
      {allowed && (canRetry || canCancel) && (
        confirming ? (
          <div role="group" aria-label={canRetry ? "Confirm retry" : "Confirm cancel"} className="flex flex-wrap items-center gap-2 rounded-md border border-border-strong p-2 text-sm">
            <span>{canRetry ? "Run this job again?" : "Stop this job? What it has done so far is thrown away, and it can be retried afterwards."}</span>
            <Button variant="primary" disabled={busy} onClick={() => void act(canRetry ? "retry" : "cancel")}>{canRetry ? "Yes, retry" : "Yes, cancel the job"}</Button>
            <Button variant="ghost" disabled={busy} onClick={() => setConfirming(false)}>Keep it as it is</Button>
          </div>
        ) : (
          <div><Button onClick={() => setConfirming(true)}>{canRetry ? "Retry job" : "Cancel job"}</Button></div>
        )
      )}
    </li>
  );
};

export const DocumentActivityView = ({ identity, vault, id, watch }: { readonly identity: Identity; readonly vault: VaultId; readonly id: string; readonly watch: ActivityWatch }) => {
  const engine = useEngine();
  const queryClient = useQueryClient();
  const { state, now } = useActivityState(watch);
  const doc = state.documents.get(id);
  const source = useQuery({ queryKey: QUERY_KEYS.source(vault, id), queryFn: async () => unwrap(await engine.getSource(vault, id)) });
  const jobs = useQuery({ queryKey: QUERY_KEYS.documentJobs(vault, id), queryFn: async () => unwrap(await engine.listDocumentJobs(vault, id, doc?.jobId ?? null)), staleTime: 0 });

  // A new job or a new state means there's more to read here than the frame said, such as an attempt's error.
  const signature = doc === undefined ? "" : `${doc.jobId ?? ""}/${doc.status}/${doc.stage}/${String(doc.attempts)}`;
  useEffect(() => {
    if (signature !== "") void queryClient.invalidateQueries({ queryKey: QUERY_KEYS.documentJobs(vault, id) });
  }, [id, queryClient, signature, vault]);

  const changed = () => {
    watch.refresh();
    void queryClient.invalidateQueries({ queryKey: QUERY_KEYS.documentJobs(vault, id) });
  };
  const detail = connectionDetail(state.connection);
  const explanation = doc === undefined ? null : statusExplanation(doc, now);
  const text = doc === undefined ? null : progressText(doc.progress);

  return (
    <div className="flex flex-col gap-4" data-document-activity={id} data-stage={doc?.stage ?? ""} data-status={doc?.status ?? ""}>
      <div className="flex flex-wrap items-center gap-3">
        <AppLink to={ingestPath(vault, { kind: "board", stage: "", status: "", text: "" })}>Back to the board</AppLink>
        <ConnectionBadge connection={state.connection} />
      </div>
      <h2 className="text-lg font-medium">{source.data?.name ?? id}</h2>
      {detail !== null && <p role="alert" className="rounded-md border border-warning bg-warning-surface p-2 text-sm text-warning">{detail}</p>}
      {state.seq !== null && doc === undefined && (
        <EmptyState title="This document isn't on the board.">It may have been erased, it may never have existed, or there may be more documents than this browser lists.</EmptyState>
      )}
      {doc !== undefined && (
        <>
          <div className="flex flex-wrap items-center gap-3 text-sm">
            <span>Stage <strong data-current-stage={doc.stage}>{stageLabel(doc.stage)}</strong></span>
            <StatusOf doc={doc} />
            {doc.stage !== "done" && <span className="text-text-muted">for {durationText(now - doc.stageSinceMs)}</span>}
            {doc.partial && <Badge tone="warning">Partly failed</Badge>}
            {doc.pendingReview > 0 && <AppLink to={`/vaults/${encodeURIComponent(vault)}/review`}>{doc.pendingReview} waiting in Review</AppLink>}
            {doc.pageCount !== null && <span className="text-text-muted">{doc.pageCount} pages</span>}
            {doc.segmentCount !== null && <span className="text-text-muted">{doc.segmentCount} segments</span>}
          </div>
          {text !== null && (
            <div data-progress-text={text} className="flex flex-col gap-1 text-sm">
              {text}
              {doc.progress !== null && doc.progress.total !== null && <ProgressBar label={text} value={doc.progress.done} max={doc.progress.total} />}
            </div>
          )}
          {explanation !== null && <p data-explanation={doc.status} className={`rounded-md border p-2 text-sm ${doc.status === "failed" ? "border-danger bg-danger-surface" : "border-border-strong bg-surface-raised"}`}>{explanation}</p>}
          <AppLink to={documentPath(vault, { kind: "source", id, tab: "view", page: null, zoom: null, claim: null, evidence: null, fromMs: null, toMs: null, rendition: null })} className={LINK_BUTTON}>Open in Documents</AppLink>
          <section aria-label="Timeline" className="flex flex-col gap-2">
            <h3 className="font-medium">Timeline</h3>
            <StageTimeline doc={doc} />
          </section>
        </>
      )}
      <section aria-label="Jobs" className="flex flex-col gap-2">
        <h3 className="font-medium">Jobs</h3>
        <QueryView query={jobs} label="this document's jobs" isEmpty={(d) => d.jobs.length === 0} empty={<EmptyState title="No jobs for this document." />}>
          {(data) => (
            <>
              {data.truncated && <p className="text-xs text-warning">This vault has many jobs, so older ones may not be listed.</p>}
              <ul className="flex flex-col gap-2">{data.jobs.map((job) => <JobCard key={job.id} vault={vault} job={job} identity={identity} onChanged={changed} />)}</ul>
            </>
          )}
        </QueryView>
      </section>
    </div>
  );
};
