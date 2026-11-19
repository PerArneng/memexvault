import { decisionAccess, decisionAccessText, defaultApproveReason, describeOp, documentPath, NO_FILTER, reviewPath, sourcePlace } from "@memex/core";
import type { DecisionOutcome } from "@memex/core";
import type { Identity, ReviewDecision, VaultId } from "@memex/types";
import { Badge, Button, Skeleton } from "@memex/ui";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { useEngine } from "../engine-context";
import { QUERY_KEYS, unwrap } from "../query";
import { EpistemicOf, outcomeNotice, ReasonForm, StateBadge, TrustOf } from "../review/review-common";
import { AppLink } from "../wiki/wiki-common";

interface ProposalCardProps {
  readonly identity: Identity;
  readonly vault: VaultId;
  readonly proposalId: string;
  /** What the validators refused, as the tool said. Such a proposal still waits in the queue. */
  readonly refused: readonly string[];
}

/**
 * A chat's proposal, read from the review API, with the same decision the review screen makes. The buttons show only to someone who may
 * decide, and the server checks again either way.
 */
export const ProposalCard = ({ identity, vault, proposalId, refused }: ProposalCardProps) => {
  const engine = useEngine();
  const queryClient = useQueryClient();
  const query = useQuery({ queryKey: QUERY_KEYS.reviewItem(vault, proposalId), queryFn: async () => unwrap(await engine.getReviewItem(vault, proposalId)), meta: { ownsErrors: true } });
  const [rejecting, setRejecting] = useState(false);
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<{ title: string; detail: string } | null>(null);
  const access = decisionAccess(identity, vault);

  const decide = async (decision: ReviewDecision) => {
    setBusy(true);
    setProblem(null);
    const outcome: DecisionOutcome = await engine.decide(vault, decision);
    setBusy(false);
    if (outcome.kind !== "done") {
      setProblem(outcomeNotice(outcome));
    } else setRejecting(false);
    // Whatever happened, the card and the review screen read again, so both show what the server holds.
    await queryClient.invalidateQueries({ queryKey: QUERY_KEYS.review(vault) });
  };

  return (
    <section aria-label="Proposed change" data-proposal={proposalId} className="flex flex-col gap-2 rounded-lg border border-border-strong bg-surface-raised p-3 text-sm">
      <header className="flex flex-wrap items-center gap-2">
        <span className="font-medium">Proposed change</span>
        {query.data !== undefined && <StateBadge state={query.data.changeset.state} />}
        <AppLink to={reviewPath(vault, { kind: "queue", filter: NO_FILTER, item: proposalId })} className="ml-auto text-xs text-accent underline">Open in review</AppLink>
      </header>
      {refused.length > 0 && (
        <p className="text-xs text-warning" data-testid="proposal-refused">The validators objected: {refused.join("; ")}</p>
      )}
      {query.isPending ? (
        <Skeleton className="h-16 w-full" />
      ) : query.isError ? (
        <p role="alert" className="text-danger">This proposal couldn't be read{query.error instanceof Error ? `: ${query.error.message}` : "."}</p>
      ) : (
        <>
          <ul className="flex flex-col gap-2">
            {query.data.changeset.ops.map((op, i) => {
              const d = describeOp(op);
              return (
                <li key={i} className="flex flex-col gap-1" data-testid="proposal-op">
                  <span className="flex flex-wrap items-center gap-1.5"><span>{d.title}</span><EpistemicOf status={d.epistemic} /></span>
                  <dl className="grid grid-cols-[auto_1fr] gap-x-2 text-xs">
                    {d.fields.map((f) => (
                      <div key={f.label} className="contents">
                        <dt className="text-text-muted">{f.label}</dt>
                        <dd className="min-w-0 break-words">{f.value}</dd>
                      </div>
                    ))}
                  </dl>
                </li>
              );
            })}
          </ul>
          {query.data.evidence.length > 0 && (
            <ul aria-label="Evidence" className="flex flex-col gap-1.5 border-t border-border pt-2">
              {query.data.evidence.map((e) => {
                const place = sourcePlace(e);
                const to = e.sourceId === null ? null : documentPath(vault, { kind: "source", id: e.sourceId, tab: "view", page: place.kind === "pages" ? place.page : null, zoom: null, claim: null, evidence: null, fromMs: null, toMs: null, rendition: null, segment: e.segmentId });
                return (
                  <li key={`${e.claimId}-${e.segmentId}`} className="flex flex-col gap-0.5">
                    <q className="break-words">{e.quote}</q>
                    <span className="flex flex-wrap items-center gap-1.5 text-xs text-text-muted">
                      <TrustOf tier={e.trustTier} />
                      {to === null ? <span>{e.sourceName ?? "Source unavailable"}</span> : <AppLink to={to}>{e.sourceName ?? e.sourceId ?? "source"}</AppLink>}
                      {e.superseded && <Badge tone="warning">Superseded</Badge>}
                    </span>
                  </li>
                );
              })}
            </ul>
          )}
          <p className="text-xs text-text-muted">
            Proposed by {query.data.changeset.proposer} for {query.data.changeset.requestedBy ?? "someone"}. Only a person can approve this, and it never auto-commits.
            {query.data.changeset.decidedBy !== null && <> Decided by <span data-testid="proposal-decided-by" className="text-text">{query.data.changeset.decidedBy}</span>.</>}
          </p>
          {query.data.changeset.state === "pending" &&
            (access.kind === "allowed" ? (
              rejecting ? (
                <ReasonForm label="Why reject it?" confirmLabel="Reject" required={true} danger={true} busy={busy} onCancel={() => setRejecting(false)} onSubmit={(reason) => void decide({ kind: "reject", id: proposalId, reason })} />
              ) : (
                <div className="flex items-center gap-2">
                  <Button variant="primary" disabled={busy} onClick={() => void decide({ kind: "approve", id: proposalId, reason: defaultApproveReason })}>Approve</Button>
                  <Button disabled={busy} onClick={() => setRejecting(true)}>Reject</Button>
                </div>
              )
            ) : (
              <p className="text-xs text-text-muted" data-testid="proposal-no-decide">{decisionAccessText(access)}</p>
            ))}
          {problem !== null && (
            <p role="alert" className="text-xs text-danger">{problem.title} {problem.detail}</p>
          )}
        </>
      )}
    </section>
  );
};
