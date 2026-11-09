import { decisionAccess, decisionAccessText, describeMutation, describeOp, reviewPath } from "@memex/core";
import type { ChangeSet, Identity, Page, VaultId } from "@memex/types";
import { Button, EmptyState } from "@memex/ui";
import { useInfiniteQuery, useQuery } from "@tanstack/react-query";
import { useMemo, useState } from "react";
import { useEngine } from "../engine-context";
import { QUERY_KEYS, unwrap } from "../query";
import { QueryView } from "../shell/query-view";
import { AppLink, Callout, formatTime } from "../wiki/wiki-common";
import { ChangeHeader, DecisionFacts, OpsList, Rationale, ValidatorsView } from "./item-view";
import { outcomeNotice, ReasonForm, StateBadge, useRefreshReview, WhoBadge } from "./review-common";

const STATES: readonly { readonly id: string; readonly label: string }[] = [
  { id: "", label: "All" },
  { id: "pending", label: "Pending" },
  { id: "committed", label: "Committed" },
  { id: "rejected", label: "Rejected" },
];

export const ChangesetList = ({ vault, state }: { readonly vault: VaultId; readonly state: string }) => {
  const engine = useEngine();
  const query = useInfiniteQuery({
    queryKey: QUERY_KEYS.changesets(vault, state),
    initialPageParam: null as string | null,
    getNextPageParam: (last: Page<ChangeSet>) => last.nextCursor,
    queryFn: async ({ pageParam }) => unwrap(await engine.listChangesets(vault, state, pageParam)),
  });
  const rows = useMemo(() => query.data?.pages.flatMap((p) => p.items) ?? [], [query.data]);
  return (
    <div className="flex flex-col gap-3">
      <nav aria-label="By state" className="flex flex-wrap gap-3 text-sm">
        {STATES.map((s) => <AppLink key={s.id} to={reviewPath(vault, { kind: "changesets", state: s.id })} aria-current={s.id === state ? "page" : undefined}>{s.label}</AppLink>)}
      </nav>
      <QueryView query={query} label="changes" isEmpty={(d) => d.pages.every((p) => p.items.length === 0)} empty={<EmptyState title="No changes here yet." />}>
        {() => (
          <div className="flex flex-col gap-2">
            <table className="w-full text-sm" data-testid="changeset-table">
              <thead className="text-left text-xs text-text-muted"><tr><th className="p-1">Change</th><th className="p-1">State</th><th className="p-1">Proposed by</th><th className="p-1">Decided by</th><th className="p-1">Proposed</th></tr></thead>
              <tbody>
                {rows.map((c) => {
                  const first = describeOp(c.ops[0]);
                  return (
                    <tr key={c.id} className="border-t border-border" data-changeset={c.id}>
                      <td className="max-w-96 truncate p-1">
                        <AppLink to={reviewPath(vault, { kind: "changeset", id: c.id })}>{first.title}{c.ops.length > 1 ? ` and ${String(c.ops.length - 1)} more` : ""}</AppLink>
                        {c.reverts !== null && <span className="ml-1 text-xs text-text-muted">A revert</span>}
                        {c.revertedBy !== null && <span className="ml-1 text-xs text-text-muted">Reverted</span>}
                      </td>
                      <td className="p-1"><StateBadge state={c.state} /></td>
                      <td className="p-1"><span className="inline-flex items-center gap-1.5"><WhoBadge machine={c.proposerIsMachine} /><span className="truncate">{c.proposer}</span></span></td>
                      <td className="p-1">{c.decidedBy ?? "–"}</td>
                      <td className="p-1 font-mono text-xs tabular-nums">{formatTime(c.proposedAt)}</td>
                    </tr>
                  );
                })}
              </tbody>
            </table>
            {query.hasNextPage && <Button className="self-start" disabled={query.isFetchingNextPage} onClick={() => void query.fetchNextPage()}>Load more</Button>}
          </div>
        )}
      </QueryView>
    </div>
  );
};

const Inverse = ({ vault, changeset, canDecide }: { readonly vault: VaultId; readonly changeset: ChangeSet; readonly canDecide: boolean }) => {
  const engine = useEngine();
  const refresh = useRefreshReview(vault);
  const [reverting, setReverting] = useState(false);
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<{ readonly title: string; readonly detail: string } | null>(null);
  const [done, setDone] = useState<ChangeSet | null>(null);
  const query = useQuery({ queryKey: QUERY_KEYS.inverse(vault, changeset.id), queryFn: async () => unwrap(await engine.getInverse(vault, changeset.id)) });
  const revert = async (reason: string) => {
    setBusy(true);
    setProblem(null);
    const outcome = await engine.revertChangeset(vault, changeset.id, reason);
    setBusy(false);
    if (outcome.kind === "done") {
      setDone(outcome.changeset);
      setReverting(false);
    } else {
      setProblem(outcomeNotice(outcome));
    }
    await refresh();
  };
  return (
    <section aria-label="Undoing it" className="flex flex-col gap-2">
      <h2 className="text-sm font-medium">Undoing it</h2>
      {done !== null && (
        <div data-testid="reverted">
          <Callout tone="info" title="Reverted. The revert is a change of its own.">
            <AppLink to={reviewPath(vault, { kind: "changeset", id: done.id })}>See the revert</AppLink>
          </Callout>
        </div>
      )}
      {problem !== null && <div data-testid="revert-problem"><Callout tone="danger" title={problem.title}>{problem.detail}</Callout></div>}
      {changeset.revertedBy !== null && done === null && (
        <p className="text-sm text-text-muted">This has been reverted. <AppLink to={reviewPath(vault, { kind: "changeset", id: changeset.revertedBy })}>See the revert</AppLink></p>
      )}
      <QueryView query={query} label="what undoing it would do">
        {(inverse) => (
          <div className="flex flex-col gap-2" data-testid="inverse">
            {!inverse.reversible && <Callout tone="warning" title="This can't be reverted now.">{inverse.blockedBecause ?? "The server gave no reason."}</Callout>}
            {inverse.reversible && (
              <>
                <p className="text-sm text-text-muted">Reverting would make {inverse.mutations.length} changes, and nothing in the history is lost:</p>
                <ul className="list-disc pl-5 text-sm" data-testid="inverse-list">
                  {/* Mutations have no ids of their own, and this list never reorders. */}
                  {inverse.mutations.map((m, i) => <li key={i} className="break-words">{describeMutation(m)}</li>)}
                </ul>
                <details className="text-xs text-text-muted"><summary>The inverse as JSON</summary><pre className="max-h-64 overflow-auto font-mono whitespace-pre-wrap">{JSON.stringify(inverse.mutations, null, 2)}</pre></details>
                {canDecide && changeset.revertedBy === null && done === null && !reverting && <Button className="self-start" onClick={() => setReverting(true)}>Revert</Button>}
                {reverting && <ReasonForm label="Why this is reverted" confirmLabel="Revert" required={true} danger={true} busy={busy} onCancel={() => setReverting(false)} onSubmit={(reason) => void revert(reason)} />}
              </>
            )}
          </div>
        )}
      </QueryView>
    </section>
  );
};

export const ChangesetDetail = ({ identity, vault, id }: { readonly identity: Identity; readonly vault: VaultId; readonly id: string }) => {
  const engine = useEngine();
  const access = decisionAccess(identity, vault);
  const query = useQuery({ queryKey: QUERY_KEYS.changeset(vault, id), queryFn: async () => unwrap(await engine.getChangeset(vault, id)) });
  const accessText = decisionAccessText(access);
  return (
    <QueryView query={query} label="this change">
      {(c) => (
        <div className="flex max-w-4xl flex-col gap-3" data-testid="changeset" data-changeset={c.id}>
          <AppLink to={reviewPath(vault, { kind: "changesets", state: "" })}>All changes</AppLink>
          <ChangeHeader changeset={c} />
          {c.state === "pending" && <p className="text-sm"><AppLink to={reviewPath(vault, { kind: "queue", filter: { extractor: "", promptVersion: "", itemType: "", proposer: "", verdict: "", sourceId: "" }, item: c.id })}>Decide it in the queue</AppLink></p>}
          {c.reverts !== null && <p className="text-sm text-text-muted">This reverts <AppLink to={reviewPath(vault, { kind: "changeset", id: c.reverts })}>an earlier change</AppLink>.</p>}
          <DecisionFacts changeset={c} />
          <Rationale text={c.rationale} />
          <OpsList ops={c.ops} label="What it does" />
          {c.originalOps !== null && <OpsList ops={c.originalOps} label="What was first proposed" />}
          <section aria-label="Validators" className="flex flex-col gap-1"><h2 className="text-sm font-medium">Validators</h2><ValidatorsView validators={c.validators} /></section>
          {c.state === "committed" && accessText !== null && <div data-testid="no-decisions"><Callout tone="warning" title="You can't revert anything here.">{accessText}</Callout></div>}
          {c.state === "committed" && <Inverse vault={vault} changeset={c} canDecide={access.kind === "allowed"} />}
        </div>
      )}
    </QueryView>
  );
};
