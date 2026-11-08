import { AUTO_COMMIT_RULES, autoCommitStateText, changeByText, changeKindText, decisionAccess, percent } from "@memex/core";
import type { AutoCommitEntry, Identity, VaultId } from "@memex/types";
import { Badge, Button, EmptyState } from "@memex/ui";
import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { useEngine } from "../engine-context";
import { QUERY_KEYS, unwrap } from "../query";
import { QueryView } from "../shell/query-view";
import { failureText, formatTime } from "../wiki/wiki-common";
import { ReasonForm, useRefreshReview } from "./review-common";

const Entry = ({ vault, entry, threshold, canDisable }: { readonly vault: VaultId; readonly entry: AutoCommitEntry; readonly threshold: number; readonly canDisable: boolean }) => {
  const engine = useEngine();
  const refresh = useRefreshReview(vault);
  const [disabling, setDisabling] = useState(false);
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const disable = async (reason: string) => {
    setBusy(true);
    setProblem(null);
    const result = await engine.disableAutoCommit(vault, entry.id, reason);
    setBusy(false);
    if (result.ok) setDisabling(false);
    else setProblem(failureText(result.failure));
    await refresh();
  };
  return (
    <>
      <tr className="border-t border-border" data-combination={entry.id}>
        <td className="p-1 break-words"><span className="font-medium">{entry.extractor}</span><br /><span className="text-xs text-text-muted">{entry.itemType}, prompt {entry.promptVersion}</span></td>
        <td className="p-1 tabular-nums" data-testid="samples">{entry.correct} correct, {entry.incorrect} wrong<br /><span className="text-xs text-text-muted">All time: {entry.totalCorrect} and {entry.totalIncorrect}</span></td>
        <td className="p-1 tabular-nums" data-testid="precision">{entry.precision === null ? "No samples" : percent(entry.precision)}</td>
        <td className="p-1 tabular-nums" data-testid="wilson">{percent(entry.wilsonBound)}</td>
        <td className="p-1" data-testid="combination-state"><Badge tone={entry.on ? "info" : "neutral"}>{entry.on ? "On" : "Off"}</Badge></td>
        <td className="p-1">{canDisable && !disabling && <Button aria-label={`Turn off ${entry.extractor} ${entry.itemType}`} onClick={() => setDisabling(true)}>Turn off</Button>}</td>
      </tr>
      <tr data-explain={entry.id}>
        <td colSpan={6} className="p-1 pb-3">
          <p className="text-sm text-text-muted" data-testid="state-text">{autoCommitStateText(entry, threshold)}</p>
          {disabling && (
            <ReasonForm
              label="Why this is turned off"
              confirmLabel="Turn off"
              required={true}
              danger={true}
              busy={busy}
              problem={problem}
              description="This stops auto-commit for this combination at once, and its counts start again. There's no way to turn it on by hand: only new samples can earn it back."
              onCancel={() => setDisabling(false)}
              onSubmit={(reason) => void disable(reason)}
            />
          )}
          {entry.history.length > 0 && (
            <details className="mt-1 text-sm" data-testid="history">
              <summary className="text-text-muted">History: {entry.history.length} changes</summary>
              <ul className="flex flex-col gap-1 pt-1">
                {entry.history.map((h, i) => (
                  // A history only grows, and only at its end.
                  <li key={i} className="flex flex-wrap items-center gap-2">
                    <Badge tone={h.kind === "disabled" ? "warning" : h.kind === "earned" ? "info" : "neutral"}>{changeKindText(h.kind)}</Badge>
                    <time className="font-mono text-xs text-text-muted tabular-nums">{formatTime(h.at)}</time>
                    <span>by {changeByText(h)}, at {h.correct} correct and {h.incorrect} wrong.</span>
                    <span className="min-w-0 break-words text-text-muted">{h.reason}</span>
                  </li>
                ))}
              </ul>
            </details>
          )}
        </td>
      </tr>
    </>
  );
};

export const AutoCommitView = ({ identity, vault }: { readonly identity: Identity; readonly vault: VaultId }) => {
  const engine = useEngine();
  const query = useQuery({ queryKey: QUERY_KEYS.autoCommit(vault), queryFn: async () => unwrap(await engine.getAutoCommitTable(vault)) });
  const canDisable = decisionAccess(identity, vault).kind === "allowed";
  return (
    <div className="flex flex-col gap-3">
      <section aria-label="How auto-commit works" className="rounded-md border border-border p-3 text-sm" data-testid="auto-commit-rules">
        <h3 className="mb-1 font-medium">How auto-commit works</h3>
        <ul className="list-disc pl-5 text-text-muted">{AUTO_COMMIT_RULES.map((r) => <li key={r}>{r}</li>)}</ul>
      </section>
      <QueryView query={query} label="the auto-commit table" isEmpty={(d) => d.entries.length === 0} empty={<EmptyState title="No combination has any samples yet.">Everything waits for a person until one earns auto-commit.</EmptyState>}>
        {(table) => (
          <div className="flex flex-col gap-1">
            <p className="text-sm">The lower bound has to reach <strong data-testid="threshold">{percent(table.threshold)}</strong>.</p>
            <table className="w-full text-sm" data-testid="auto-commit-table">
              <thead className="text-left text-xs text-text-muted"><tr><th className="p-1">Combination</th><th className="p-1">Samples</th><th className="p-1">Precision</th><th className="p-1">Wilson lower bound</th><th className="p-1">State</th><th className="p-1"><span className="sr-only">Actions</span></th></tr></thead>
              <tbody>{table.entries.map((e) => <Entry key={e.id} vault={vault} entry={e} threshold={table.threshold} canDisable={canDisable} />)}</tbody>
            </table>
          </div>
        )}
      </QueryView>
    </div>
  );
};
