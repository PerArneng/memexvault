import { auditPath, auditRunStatus, budgetUsedText, checkName, checkStatusOf, documentPath, formatCount, isFinding, NO_FILTER, noReviewItemText, outcomeOf, reviewPath, runIsOpen, tokensTotal, triggerText } from "@memex/core";
import type { AuditCheck, AuditRun, VaultId } from "@memex/types";
import { EmptyState } from "@memex/ui";
import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { useEngine } from "../engine-context";
import { QUERY_KEYS, unwrap } from "../query";
import { QueryView } from "../shell/query-view";
import { AppLink, formatTime } from "../wiki/wiki-common";
import { Fact, OPEN_RUN_POLL_MS, RunStateBadge, ToneBadge } from "./audit-common";

const Summary = ({ run }: { readonly run: AuditRun }) => {
  const status = auditRunStatus(run);
  return (
    <section aria-label="Run" className="flex flex-col gap-2 rounded-lg border border-border p-3">
      <div className="flex flex-wrap items-center gap-2">
        <RunStateBadge run={run} />
        <span className="text-sm text-text-muted">{triggerText(run.trigger)}, started {formatTime(run.createdAt)}{run.finishedAt === null ? "" : `, finished ${formatTime(run.finishedAt)}`}</span>
      </div>
      {status.explain !== null && <p className="text-sm text-text-muted" data-testid="run-explain">{status.explain}</p>}
      {run.stopDetail !== null && <p className="text-sm whitespace-pre-wrap break-words text-text-muted" data-testid="stop-detail">{run.stopDetail}</p>}
      <dl className="grid grid-cols-2 gap-3 text-sm md:grid-cols-4">
        <Fact label="Seed" testId="run-seed"><span className="font-mono">{run.seed}</span></Fact>
        <Fact label="Sample" testId="run-sample">{formatCount(run.sampled)} of {formatCount(run.sampleSize)} asked, from {formatCount(run.population)} claims</Fact>
        <Fact label="Budget used" testId="run-budget">{budgetUsedText(run)}</Fact>
        <Fact label="Model">{run.model ?? "None: only deterministic checks ran"}</Fact>
        <Fact label="Passed" testId="run-passed">{formatCount(run.passed)}</Fact>
        <Fact label="Suspect" testId="run-suspect">{formatCount(run.suspect)}</Fact>
        <Fact label="Failed" testId="run-failed">{formatCount(run.failed)}</Fact>
        <Fact label="LLM calls">{formatCount(run.llmCalls)}, {formatCount(run.llmSkipped)} claims without LLM checks</Fact>
        <Fact label="Tokens">{formatCount(run.tokens.input)} in, {formatCount(run.tokens.output)} out, {formatCount(run.tokens.thinking)} thinking</Fact>
        <Fact label="Auto-commit turned off">{run.revoked.length === 0 ? "None" : run.revoked.join(", ")}</Fact>
        <Fact label="Sent back to review">{formatCount(run.siblingsRequeued)}, with {formatCount(run.siblingsRemaining)} still to go</Fact>
        {run.missedSlots > 0 && <Fact label="Missed slots">{formatCount(run.missedSlots)}</Fact>}
      </dl>
      {run.vaultFindings.length > 0 && (
        <div className="flex flex-col gap-1">
          <h2 className="text-sm font-medium">About the vault as a whole</h2>
          <ul className="flex flex-col gap-1 text-sm" data-testid="vault-findings">
            {run.vaultFindings.map((f, i) => {
              const s = checkStatusOf(f.status);
              // A run's vault findings never change order.
              return <li key={i} className="flex flex-wrap items-start gap-2"><ToneBadge tone={s.tone}>{s.label}</ToneBadge><span>{checkName(f.check)}</span><span className="min-w-0 whitespace-pre-wrap break-words text-text-muted">{f.detail}</span></li>;
            })}
          </ul>
        </div>
      )}
    </section>
  );
};

/** The claim, its sources and what it says, read when someone opens a finding. */
const ClaimSources = ({ vault, claim }: { readonly vault: VaultId; readonly claim: string }) => {
  const engine = useEngine();
  const query = useQuery({ queryKey: QUERY_KEYS.lineage(vault, claim), queryFn: async () => unwrap(await engine.getLineage(vault, claim)) });
  return (
    <QueryView query={query} label="the claim">
      {(l) => (
        <div className="flex flex-col gap-1 text-sm" data-testid="claim-sources">
          <p className="break-words"><span className="text-text-muted">Claim:</span> {l.subject ?? ""} {l.predicate ?? ""} <strong>{l.value}</strong></p>
          <ul className="flex flex-col gap-1">
            {l.evidence.map((e) => (
              <li key={e.evidenceId} className="flex flex-wrap gap-2">
                <AppLink to={documentPath(vault, { kind: "source", id: e.sourceId, tab: "view", page: null, zoom: null, claim, evidence: e.evidenceId, fromMs: null, toMs: null, rendition: null, segment: null })}>{e.sourceName}</AppLink>
                <q className="min-w-0 break-words text-text-muted">{e.quote}</q>
              </li>
            ))}
          </ul>
        </div>
      )}
    </QueryView>
  );
};

const CheckRow = ({ vault, check }: { readonly vault: VaultId; readonly check: AuditCheck }) => {
  const [open, setOpen] = useState(false);
  const outcome = outcomeOf(check.outcome);
  const noItem = noReviewItemText(check);
  return (
    <>
      <tr className="border-t border-border align-top" data-check={check.claimId} data-outcome={check.outcome}>
        <td className="p-1 tabular-nums">{check.rank + 1}</td>
        <td className="p-1"><ToneBadge tone={outcome.tone} testId="check-outcome">{outcome.label}</ToneBadge></td>
        <td className="p-1 break-all font-mono text-xs"><AppLink to={documentPath(vault, { kind: "lineage", claim: check.claimId, evidence: null })}>{check.claimId}</AppLink></td>
        <td className="p-1 text-xs">{check.combination === null ? "A person's claim" : `${check.combination.extractor}, ${check.combination.itemType}`}{check.autoCommitted ? ", auto-committed" : ""}</td>
        <td className="p-1">
          <ul className="flex flex-col gap-0.5">
            {check.checks.map((c) => {
              const s = checkStatusOf(c.status);
              return <li key={c.check} className="flex flex-wrap items-center gap-1 text-xs"><ToneBadge tone={s.tone}>{s.label}</ToneBadge>{checkName(c.check)}</li>;
            })}
          </ul>
        </td>
        <td className="p-1 text-sm">
          {check.changesetId !== null ? (
            <AppLink to={reviewPath(vault, { kind: "queue", filter: NO_FILTER, item: check.changesetId })} data-link-state="review-item">Review item</AppLink>
          ) : noItem !== null ? <span className="text-xs text-text-muted">None</span> : null}
        </td>
        <td className="p-1"><button type="button" className="text-accent underline" aria-expanded={open} onClick={() => setOpen(!open)}>{open ? "Hide" : "Details"}</button></td>
      </tr>
      {open && (
        <tr>
          <td colSpan={7} className="p-2 pb-4">
            <div className="flex flex-col gap-2 rounded-md bg-surface-raised p-2">
              {noItem !== null && <p className="text-sm text-text-muted">{noItem}</p>}
              <ClaimSources vault={vault} claim={check.claimId} />
              <ul className="flex flex-col gap-1 text-sm" data-testid="check-details">
                {check.checks.map((c) => (
                  <li key={c.check} className="flex flex-col">
                    <span className="font-medium">{checkName(c.check)}{c.cause === null ? "" : `: ${c.cause}`}{c.model === null ? "" : `, by ${c.model}`}{c.tokens === null ? "" : `, ${formatCount(tokensTotal(c.tokens))} tokens`}</span>
                    {/* A document's or a model's words, so plain text. */}
                    <span className="whitespace-pre-wrap break-words text-text-muted">{c.detail}</span>
                  </li>
                ))}
              </ul>
            </div>
          </td>
        </tr>
      )}
    </>
  );
};

export const RunDetail = ({ vault, id, findingsOnly }: { readonly vault: VaultId; readonly id: string; readonly findingsOnly: boolean }) => {
  const engine = useEngine();
  const run = useQuery({
    queryKey: QUERY_KEYS.auditRun(vault, id),
    queryFn: async () => unwrap(await engine.getAuditRun(vault, id)),
    refetchInterval: (q) => (q.state.data !== undefined && runIsOpen(q.state.data.run) ? OPEN_RUN_POLL_MS : false),
  });
  const open = run.data !== undefined && runIsOpen(run.data.run);
  const checks = useQuery({
    queryKey: [...QUERY_KEYS.auditChecks(vault, id, findingsOnly), run.data?.run.state ?? "", run.data?.run.passed ?? 0, run.data?.run.suspect ?? 0, run.data?.run.failed ?? 0],
    queryFn: async () => unwrap(await engine.listAuditChecks(vault, id, findingsOnly)),
    enabled: run.data !== undefined,
    placeholderData: (previous) => previous,
  });
  return (
    <div className="flex flex-col gap-3">
      <AppLink to={auditPath(vault, { kind: "runs" })}>All runs</AppLink>
      <QueryView query={run} label="this run">
        {(detail) => (
          <>
            <Summary run={detail.run} />
            <section aria-label="Sample" className="flex flex-col gap-1">
              <h2 className="font-medium">Sample, in rank order</h2>
              {detail.sample.length === 0 ? (
                <p className="text-sm text-text-muted">{open ? "Not sampled yet." : "Nothing was sampled: the vault had no claims to check."}</p>
              ) : (
                <ol className="flex flex-wrap gap-x-3 gap-y-1 font-mono text-xs text-text-muted" data-testid="sample">{detail.sample.map((c) => <li key={c}>{c}</li>)}</ol>
              )}
            </section>
          </>
        )}
      </QueryView>
      <section aria-label="Findings" className="flex flex-col gap-2">
        <div className="flex flex-wrap items-center gap-3">
          <h2 className="font-medium">{findingsOnly ? "Findings" : "Every sampled claim"}</h2>
          <AppLink to={auditPath(vault, { kind: "run", id, findingsOnly: !findingsOnly })}>{findingsOnly ? "Show every sampled claim" : "Show findings only"}</AppLink>
        </div>
        <QueryView
          query={checks}
          label="what this run found"
          isEmpty={(list) => list.length === 0}
          empty={<EmptyState title={findingsOnly ? (open ? "No findings yet." : "This run found nothing wrong.") : open ? "Nothing checked yet." : "This run checked no claims."} />}
        >
          {(list) => (
            <table className="w-full text-sm" data-testid="checks-table">
              <thead className="text-left text-xs text-text-muted"><tr><th className="p-1">Rank</th><th className="p-1">Outcome</th><th className="p-1">Claim</th><th className="p-1">Made by</th><th className="p-1">Checks</th><th className="p-1">Review</th><th className="p-1"><span className="sr-only">Details</span></th></tr></thead>
              <tbody>{list.map((c) => <CheckRow key={c.id} vault={vault} check={c} />)}</tbody>
            </table>
          )}
        </QueryView>
        {checks.data !== undefined && !findingsOnly && <p className="text-xs text-text-muted">{formatCount(checks.data.filter(isFinding).length)} of {formatCount(checks.data.length)} are findings.</p>}
      </section>
    </div>
  );
};
