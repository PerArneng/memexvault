import { adminPath, compareErasure, NO_FILTER, removedByTable, reviewPath } from "@memex/core";
import type { DocumentSource, ErasureImpact, ErasureRecord, VaultId } from "@memex/types";
import { Badge, Button, Input } from "@memex/ui";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { useEngine } from "../engine-context";
import { QUERY_KEYS, unwrap } from "../query";
import { QueryView } from "../shell/query-view";
import { AppLink } from "../wiki/wiki-common";
import { Callout, ConfirmPanel, formatWhen, KeyValues, Panel } from "./admin-common";

/** What an erasure takes, by table, and what it leaves flagged. The same view serves the preview and the record, so they read alike. */
export const ImpactView = ({ impact, label }: { readonly impact: ErasureImpact; readonly label: string }) => (
  <section aria-label={label} className="flex flex-col gap-2">
    <KeyValues
      label={`${label}: totals`}
      rows={[
        ["Versions of the document", String(impact.sources.length)],
        ["Rows removed", <span data-testid="removed-count">{impact.removed.length}</span>],
        ["Rows rewritten", <span data-testid="rewritten-count">{impact.rewritten.length}</span>],
        ["Claims flagged", <span data-testid="flagged-claims-count">{impact.flaggedClaims.length}</span>],
        ["Pages flagged", <span data-testid="flagged-pages-count">{impact.flaggedPages.length}</span>],
      ]}
    />
    <table aria-label={`${label}: removed by table`} className="w-full max-w-md text-left text-sm">
      <thead className="text-xs text-text-muted"><tr><th className="py-1 pr-3 font-normal">Table</th><th className="font-normal">Rows removed</th></tr></thead>
      <tbody>
        {removedByTable(impact).map((r) => <tr key={r.table} className="border-t border-border"><td className="py-1 pr-3 font-mono text-xs">{r.table}</td><td className="tabular-nums">{r.count}</td></tr>)}
      </tbody>
    </table>
    <p className="text-xs text-text-muted">Rewritten rows stay, with what came from the document taken out. Flagged claims stand on other evidence too, and flagged pages link to the document or to a claim that goes; their text isn't changed.</p>
  </section>
);

/** Follows a proposal to its decision and, once approved, to the erasure's record, then compares that with the preview it was proposed from. */
const AfterProposal = ({ vault, changesetId, preview }: { readonly vault: VaultId; readonly changesetId: string; readonly preview: ErasureImpact }) => {
  const engine = useEngine();
  const changeset = useQuery({
    queryKey: ["admin", "erasure-changeset", vault, changesetId],
    queryFn: async () => unwrap(await engine.getChangeset(vault, changesetId)),
    refetchInterval: (q) => (q.state.data?.state === "pending" || q.state.data === undefined ? 2000 : false),
    meta: { ownsErrors: true },
  });
  const committed = changeset.data?.state === "committed";
  const records = useQuery({
    queryKey: QUERY_KEYS.erasures(vault),
    queryFn: async () => unwrap(await engine.listErasures(vault)),
    enabled: committed,
    refetchInterval: (q) => (committed && q.state.data?.some((r) => r.changesetId === changesetId) !== true ? 2000 : false),
  });
  const record = records.data?.find((r) => r.changesetId === changesetId) ?? null;
  const comparison = record === null ? null : compareErasure(preview, record.impact);
  return (
    <div className="flex flex-col gap-3" data-testid="erasure-progress" data-state={record !== null ? "erased" : (changeset.data?.state ?? "unknown")}>
      {changeset.data?.state === "pending" && (
        <Callout tone="info" title="The erasure is waiting for a person to approve it in Review. Nothing has been removed yet.">
          An erasure always needs a person's approval, whoever proposes it. <AppLink to={reviewPath(vault, { kind: "queue", filter: NO_FILTER, item: changesetId })}>Open it in Review</AppLink>
        </Callout>
      )}
      {changeset.data?.state === "rejected" && <Callout tone="warning" title={`The erasure was rejected${changeset.data.decisionReason === null ? "" : `: ${changeset.data.decisionReason}`}. Nothing was removed.`} />}
      {committed && record === null && <Callout tone="info" title="Approved. The erasure is running, and its report appears here when it's done." />}
      {record !== null && comparison !== null && (
        <section aria-label="Erasure report" className="flex flex-col gap-2">
          <h3 className="font-medium">What was removed</h3>
          <KeyValues label="The erasure" rows={[["Approved by", record.actor], ["At", formatWhen(record.timeMs)], ["Event", String(record.seq)]]} />
          {comparison.same ? (
            <Callout tone="info" title="The result matches the preview exactly." />
          ) : (
            <Callout tone="warning" title="The result differs from the preview, as the vault changed in between.">
              <ul className="list-disc pl-5">{comparison.differences.slice(0, 50).map((d) => <li key={d}>{d}</li>)}</ul>
            </Callout>
          )}
          <ImpactView impact={record.impact} label="Result" />
        </section>
      )}
    </div>
  );
};

const EraseSource = ({ vault, source }: { readonly vault: VaultId; readonly source: DocumentSource }) => {
  const engine = useEngine();
  const queryClient = useQueryClient();
  const preview = useQuery({ queryKey: QUERY_KEYS.erasurePreview(vault, source.id), queryFn: async () => unwrap(await engine.getErasurePreview(vault, source.id)) });
  const [rationale, setRationale] = useState("");
  const [confirming, setConfirming] = useState(false);
  const [proposed, setProposed] = useState<{ readonly changesetId: string; readonly preview: ErasureImpact } | null>(null);
  const [refusal, setRefusal] = useState<string | null>(null);
  return (
    <Panel title={`Erase ${source.name}`} description="Erasure removes the document, every version of it, and everything derived only from it, from the tables, the indexes, the files and the event log. It can't be undone.">
      <QueryView query={preview} label="the erasure's preview">
        {(impact) => (
          <div className="flex flex-col gap-3">
            <ImpactView impact={impact} label="Preview" />
            {proposed === null ? (
              confirming ? (
                <ConfirmPanel
                  title={`Propose erasing ${source.name}`}
                  description="This sends the erasure for approval. A person then has to approve it in Review before anything is removed."
                  typed={{ expected: source.name, label: `Type the document's name, ${source.name}, to confirm` }}
                  confirmLabel="Propose the erasure"
                  run={async () => {
                    const result = await engine.proposeErasure(vault, source.id, rationale.trim());
                    if (!result.ok) return result.failure;
                    if (result.value.changesetId === null || result.value.state === "rejected") {
                      setRefusal(result.value.refusals.join(" ") || "The server refused the erasure.");
                      return null;
                    }
                    setProposed({ changesetId: result.value.changesetId, preview: impact });
                    await queryClient.invalidateQueries({ queryKey: QUERY_KEYS.review(vault) });
                    return null;
                  }}
                  onDone={() => setConfirming(false)}
                  onCancel={() => setConfirming(false)}
                />
              ) : (
                <div className="flex flex-col gap-2">
                  <Input label="Why it's being erased, for the person who approves it" value={rationale} onChange={(e) => setRationale(e.target.value)} />
                  <div><Button variant="danger" disabled={rationale.trim() === ""} onClick={() => setConfirming(true)}>Erase…</Button></div>
                </div>
              )
            ) : (
              <AfterProposal vault={vault} changesetId={proposed.changesetId} preview={proposed.preview} />
            )}
            {refusal !== null && <Callout tone="danger" title={refusal} />}
          </div>
        )}
      </QueryView>
    </Panel>
  );
};

const ErasureHistory = ({ vault }: { readonly vault: VaultId }) => {
  const engine = useEngine();
  const query = useQuery({ queryKey: QUERY_KEYS.erasures(vault), queryFn: async () => unwrap(await engine.listErasures(vault)) });
  return (
    <QueryView query={query} label="erasures" isEmpty={(rows) => rows.length === 0} empty={<p className="text-sm text-text-muted">No document has been erased from this vault.</p>}>
      {(rows: readonly ErasureRecord[]) => (
        <table className="w-full text-left text-sm">
          <thead className="text-xs text-text-muted"><tr><th className="py-1 pr-3 font-normal">Document</th><th className="pr-3 font-normal">When</th><th className="pr-3 font-normal">Approved by</th><th className="pr-3 font-normal">Removed</th><th className="font-normal">Flagged</th></tr></thead>
          <tbody>
            {rows.map((r) => (
              <tr key={r.seq} className="border-t border-border">
                <td className="py-1.5 pr-3 font-mono text-xs">{r.impact.logicalId}</td>
                <td className="pr-3 font-mono text-xs">{formatWhen(r.timeMs)}</td>
                <td className="pr-3">{r.actor}</td>
                <td className="pr-3 tabular-nums">{r.impact.removed.length} rows</td>
                <td className="tabular-nums">{r.impact.flaggedClaims.length} claims, {r.impact.flaggedPages.length} pages</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </QueryView>
  );
};

const SourcePicker = ({ vault }: { readonly vault: VaultId }) => {
  const engine = useEngine();
  const query = useQuery({ queryKey: QUERY_KEYS.sources(vault), queryFn: async () => unwrap(await engine.listSources(vault)) });
  const [filter, setFilter] = useState("");
  return (
    <QueryView query={query} label="documents" isEmpty={(list) => list.sources.length === 0}>
      {(list) => {
        const shown = list.sources.filter((s) => s.name.toLowerCase().includes(filter.trim().toLowerCase())).slice(0, 200);
        return (
          <div className="flex flex-col gap-2">
            <Input label="Find a document" value={filter} onChange={(e) => setFilter(e.target.value)} />
            <ul className="flex flex-col gap-1 text-sm">
              {shown.map((s) => (
                <li key={s.id} className="flex items-center gap-2">
                  <AppLink to={adminPath(vault, "erasure", s.id)}>{s.name}</AppLink>
                  <Badge>{s.mediaType}</Badge>
                  {s.versionNo > 1 && <span className="text-xs text-text-muted">version {s.versionNo}</span>}
                </li>
              ))}
            </ul>
            {list.truncated && <p className="text-xs text-text-muted">Only the first documents are listed here. Open a document's erasure from the address /admin/erasure/&lt;its id&gt;.</p>}
          </div>
        );
      }}
    </QueryView>
  );
};

export const ErasureTab = ({ vault, id }: { readonly vault: VaultId; readonly id: string | null }) => {
  const engine = useEngine();
  const source = useQuery({ queryKey: QUERY_KEYS.source(vault, id ?? ""), queryFn: async () => unwrap(await engine.getSource(vault, id ?? "")), enabled: id !== null });
  if (id !== null) {
    return (
      <div className="flex flex-col gap-3">
        <QueryView query={source} label="this document">
          {(s) => <EraseSource key={s.id} vault={vault} source={s} />}
        </QueryView>
        <AppLink to={adminPath(vault, "erasure")}>Erase another document</AppLink>
      </div>
    );
  }
  return (
    <div className="flex flex-col gap-4">
      <Panel title="Erase a document" description="Choose the document. The next page shows exactly what would go before anything is proposed.">
        <SourcePicker vault={vault} />
      </Panel>
      <Panel title="Erasures in this vault">
        <ErasureHistory vault={vault} />
      </Panel>
    </div>
  );
};
