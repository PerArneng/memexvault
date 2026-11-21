import { adminPath, isOperationOpen, needsTypedConfirmation, operationDef, operationFraction, operationRequest, operationRequestProblem, operationStateText, OPERATIONS, settingRows } from "@memex/core";
import type { OperationKind, OperationRequest, UploadFile } from "@memex/core";
import type { Identity, ImportRun, OperationJob, VaultId } from "@memex/types";
import { Badge, Button, Input } from "@memex/ui";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { useEngine } from "../engine-context";
import { QUERY_KEYS, unwrap } from "../query";
import { QueryView } from "../shell/query-view";
import { AppLink, useGo } from "../wiki/wiki-common";
import { Callout, ConfirmPanel, formatWhen, KeyValues, Panel } from "./admin-common";

const STATE_TONE: Readonly<Record<string, "neutral" | "success" | "danger" | "info" | "warning">> = { queued: "neutral", running: "info", done: "success", dead: "danger", cancelled: "warning" };

const ProgressBar = ({ job }: { readonly job: OperationJob }) => {
  const fraction = operationFraction(job);
  return (
    <div className="flex flex-col gap-1">
      <span className="text-sm" data-testid="operation-state">{operationStateText(job)}</span>
      {fraction !== null && (
        <div role="progressbar" aria-label="Progress" aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(fraction * 100)} className="h-1.5 w-full max-w-sm overflow-hidden rounded-full bg-neutral-surface">
          <div className="h-full bg-accent" style={{ width: `${String(Math.round(fraction * 100))}%` }} />
        </div>
      )}
    </div>
  );
};

/** A report is the server's own data, laid out the way settings are, so a new field needs no change here. */
export const ReportView = ({ report, label }: { readonly report: Readonly<Record<string, unknown>>; readonly label: string }) => (
  <table aria-label={label} className="w-full text-left text-sm">
    <tbody>
      {settingRows(report).map((r) => (
        <tr key={r.path} className="border-t border-border">
          <th scope="row" className="py-1 pr-3 font-mono text-xs font-normal text-text-muted">{r.path}</th>
          <td className="break-all font-mono text-xs">{r.value}</td>
        </tr>
      ))}
    </tbody>
  </table>
);

const OperationDetail = ({ vault, id }: { readonly vault: VaultId; readonly id: string }) => {
  const engine = useEngine();
  const query = useQuery({
    queryKey: [...QUERY_KEYS.operations(vault), id],
    queryFn: async () => unwrap(await engine.getOperation(vault, id)),
    // Follows the job while it runs. Operations write their progress only between steps, so once a second is plenty.
    refetchInterval: (q) => (q.state.data === undefined || isOperationOpen(q.state.data) ? 1000 : false),
    meta: { ownsErrors: true },
  });
  return (
    <Panel title="Operation">
      <QueryView query={query} label="this operation">
        {(job) => (
          <div className="flex flex-col gap-3" data-testid="operation-detail" data-state={job.state}>
            <KeyValues label="This operation" rows={[["Operation", operationDef(job.op)?.label ?? job.op], ["Job", <span className="font-mono">{job.id}</span>], ["Started", formatWhen(job.createdAtMs)], ["Finished", job.finishedAtMs === null ? "not yet" : formatWhen(job.finishedAtMs)]]} />
            <ProgressBar job={job} />
            {job.state === "done" && job.op === "export" && (
              <div><a className="text-accent underline" href={`/api/v1/vaults/${encodeURIComponent(vault)}/operations/${encodeURIComponent(job.id)}/archive`} download>Download the archive</a></div>
            )}
            {job.report !== null && (
              <section aria-label="Report" className="flex flex-col gap-1">
                <h3 className="text-sm font-medium">Report</h3>
                <ReportView report={job.report} label="What it found" />
              </section>
            )}
          </div>
        )}
      </QueryView>
      <AppLink to={adminPath(vault, "operations")}>All operations</AppLink>
    </Panel>
  );
};

const OptionsFor = ({ request, onChange }: { readonly request: OperationRequest; readonly onChange: (r: OperationRequest) => void }) => {
  const def = operationDef(request.op);
  const [at, setAt] = useState("");
  const [grace, setGrace] = useState("");
  const number = (text: string): number | null => (text.trim() === "" ? null : Number(text));
  return (
    <div className="flex flex-wrap items-end gap-3 text-sm">
      {def?.needsIndex === true && (
        <fieldset className="flex items-center gap-3">
          <legend className="sr-only">Index</legend>
          {(["keyword", "vector"] as const).map((index) => (
            <label key={index} className="inline-flex items-center gap-1"><input type="radio" name="operation-index" checked={request.index === index} onChange={() => onChange({ ...request, index })} /> {index === "keyword" ? "Keyword index" : "Vector index"}</label>
          ))}
        </fieldset>
      )}
      {request.op === "reconcile" && (
        <>
          <label className="inline-flex items-center gap-1"><input type="checkbox" checked={request.deep} onChange={(e) => onChange({ ...request, deep: e.target.checked })} /> Re-hash every blob</label>
          <label className="inline-flex items-center gap-1"><input type="checkbox" checked={request.repair} onChange={(e) => onChange({ ...request, repair: e.target.checked })} /> Repair an index that differs</label>
          <Input label="Replay up to event (blank for the head)" inputMode="numeric" value={at} onChange={(e) => {
            setAt(e.target.value);
            onChange({ ...request, at: number(e.target.value) });
          }} />
        </>
      )}
      {request.op === "sweep" && (
        <Input label="Grace in seconds (blank for the server's)" inputMode="numeric" value={grace} onChange={(e) => {
          setGrace(e.target.value);
          onChange({ ...request, graceS: number(e.target.value) });
        }} />
      )}
    </div>
  );
};

const StartOperation = ({ vault, vaultName }: { readonly vault: VaultId; readonly vaultName: string }) => {
  const engine = useEngine();
  const queryClient = useQueryClient();
  const go = useGo();
  const [request, setRequest] = useState<OperationRequest>(operationRequest("reconcile"));
  const [confirming, setConfirming] = useState(false);
  const problem = operationRequestProblem(request);
  const def = operationDef(request.op);
  const typed = needsTypedConfirmation(request);
  const choose = (op: OperationKind) => {
    setConfirming(false);
    setRequest(operationRequest(op));
  };
  return (
    <div className="flex flex-col gap-3">
      <fieldset className="flex flex-col gap-1">
        <legend className="mb-1 text-xs text-text-muted">Operation</legend>
        {OPERATIONS.map((o) => (
          <label key={o.op} className="flex items-start gap-2 text-sm">
            <input type="radio" name="operation" className="mt-1" checked={request.op === o.op} onChange={() => choose(o.op)} />
            <span><span className="text-text">{o.label}</span>{o.destructive && <> <Badge tone="danger">destructive</Badge></>} <span className="text-text-muted">{o.summary}</span></span>
          </label>
        ))}
      </fieldset>
      <OptionsFor key={request.op} request={request} onChange={setRequest} />
      {problem !== null && <Callout tone="warning" title={problem} />}
      {confirming ? (
        <ConfirmPanel
          title={`${def?.label ?? request.op} in ${vaultName}`}
          danger={typed}
          description={def?.summary ?? ""}
          {...(typed ? { typed: { expected: vaultName, label: `Type the vault's name, ${vaultName}, to confirm` } } : {})}
          confirmLabel={`Start ${(def?.label ?? request.op).toLowerCase()}`}
          run={async () => {
            const result = await engine.startOperation(vault, request);
            if (!result.ok) return result.failure;
            await queryClient.invalidateQueries({ queryKey: QUERY_KEYS.operations(vault) });
            go(adminPath(vault, "operations", result.value.id));
            return null;
          }}
          onDone={() => setConfirming(false)}
          onCancel={() => setConfirming(false)}
        />
      ) : (
        <div><Button variant="primary" disabled={problem !== null} onClick={() => setConfirming(true)}>Start…</Button></div>
      )}
    </div>
  );
};

const OperationList = ({ vault }: { readonly vault: VaultId }) => {
  const engine = useEngine();
  const query = useQuery({
    queryKey: QUERY_KEYS.operations(vault),
    queryFn: async () => unwrap(await engine.listOperations(vault)),
    refetchInterval: (q) => (q.state.data?.some(isOperationOpen) === true ? 2000 : false),
  });
  return (
    <QueryView query={query} label="operations" isEmpty={(rows) => rows.length === 0}>
      {(rows) => (
        <table className="w-full text-left text-sm">
          <thead className="text-xs text-text-muted"><tr><th className="py-1 pr-3 font-normal">Operation</th><th className="pr-3 font-normal">State</th><th className="pr-3 font-normal">Started</th></tr></thead>
          <tbody>
            {rows.map((job) => (
              <tr key={job.id} data-operation={job.id} className="border-t border-border">
                <td className="py-1.5 pr-3"><AppLink to={adminPath(vault, "operations", job.id)}>{operationDef(job.op)?.label ?? job.op}</AppLink></td>
                <td className="pr-3"><Badge tone={STATE_TONE[job.state] ?? "neutral"}>{job.state}</Badge> <span className="text-text-muted">{operationStateText(job)}</span></td>
                <td className="pr-3 font-mono text-xs">{formatWhen(job.createdAtMs)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </QueryView>
  );
};

const ImportStatus = ({ id }: { readonly id: string }) => {
  const engine = useEngine();
  const query = useQuery({
    queryKey: ["admin", "import", id],
    queryFn: async () => unwrap(await engine.getImport(id)),
    refetchInterval: (q) => (q.state.data?.state === "running" || q.state.data === undefined ? 1000 : false),
    meta: { ownsErrors: true },
  });
  return (
    <QueryView query={query} label="the import">
      {(run: ImportRun) => (
        <div className="flex flex-col gap-2" data-testid="import-state" data-state={run.state}>
          <p className="text-sm">{run.state === "running" ? `Running, step ${String(run.progress.step)} of ${String(run.progress.of)}: ${run.progress.label}` : run.state === "done" ? "Done." : `Failed: ${run.error ?? "no reason given"}`}</p>
          {run.report !== null && <ReportView report={run.report} label="What was restored" />}
        </div>
      )}
    </QueryView>
  );
};

const REPLACE_WORDS = "replace the vault";

/** Restores an archive. A server admin's, as the vault it makes may not exist yet. */
const ImportPanel = () => {
  const engine = useEngine();
  const queryClient = useQueryClient();
  const [file, setFile] = useState<UploadFile | null>(null);
  const [newId, setNewId] = useState(true);
  const [replace, setReplace] = useState(false);
  const [name, setName] = useState("");
  const [confirming, setConfirming] = useState(false);
  const [started, setStarted] = useState<string | null>(null);
  const run = async () => {
    if (file === null) return null;
    const result = await engine.startImport(file, { newId, replace: !newId && replace, name: name.trim() === "" ? null : name });
    if (!result.ok) return result.failure;
    setStarted(result.value.id);
    await queryClient.invalidateQueries({ queryKey: QUERY_KEYS.vaults });
    return null;
  };
  return (
    <div className="flex flex-col gap-3">
      <div className="flex flex-col gap-1">
        <label htmlFor="import-file" className="text-xs text-text-muted">Archive from an export</label>
        <input id="import-file" type="file" className="text-sm" onChange={(e) => {
          setFile(e.target.files?.[0] ?? null);
          setStarted(null);
        }} />
      </div>
      <fieldset className="flex flex-col gap-1 text-sm">
        <legend className="sr-only">Where it goes</legend>
        <label className="inline-flex items-center gap-1"><input type="radio" name="import-target" checked={newId} onChange={() => setNewId(true)} /> As a new vault, under a new id</label>
        <label className="inline-flex items-center gap-1"><input type="radio" name="import-target" checked={!newId} onChange={() => setNewId(false)} /> Under the archive's own id</label>
        {!newId && <label className="ml-5 inline-flex items-center gap-1"><input type="checkbox" checked={replace} onChange={(e) => setReplace(e.target.checked)} /> Replace that vault if it exists <Badge tone="danger">destructive</Badge></label>}
      </fieldset>
      <Input label="Name for the restored vault (blank keeps the archive's)" value={name} onChange={(e) => setName(e.target.value)} />
      {confirming ? (
        <ConfirmPanel
          title={!newId && replace ? "Import, replacing an existing vault" : "Import this archive"}
          danger={!newId && replace}
          description={!newId && replace ? "If a vault with the archive's id exists, everything in it is overwritten with the archive's contents." : "This makes a vault from the archive. Nothing that exists is changed."}
          {...(!newId && replace ? { typed: { expected: REPLACE_WORDS, label: `Type "${REPLACE_WORDS}" to confirm` } } : {})}
          confirmLabel="Import"
          run={run}
          onDone={() => setConfirming(false)}
          onCancel={() => setConfirming(false)}
        />
      ) : (
        <div><Button variant="primary" disabled={file === null} onClick={() => setConfirming(true)}>Import…</Button></div>
      )}
      {started !== null && <ImportStatus id={started} />}
      {file === null && <p className="text-xs text-text-muted">Choose an archive file first.</p>}
    </div>
  );
};

export const OperationsTab = ({ identity, vault, id }: { readonly identity: Identity; readonly vault: VaultId; readonly id: string | null }) => {
  const engine = useEngine();
  const vaultQuery = useQuery({ queryKey: QUERY_KEYS.vault(vault), queryFn: async () => unwrap(await engine.getVault(vault)) });
  if (id !== null) return <OperationDetail key={id} vault={vault} id={id} />;
  return (
    <div className="flex flex-col gap-4">
      <Panel title="Run an operation" description="Each runs as a job in this vault, one of a kind at a time, with its progress and report here.">
        <QueryView query={vaultQuery} label="this vault">
          {(v) => <StartOperation vault={vault} vaultName={v.name} />}
        </QueryView>
      </Panel>
      <Panel title="Operations in this vault">
        <OperationList vault={vault} />
      </Panel>
      {identity.principal.serverAdmin && (
        <Panel title="Import" description="Restores an archive from an export into a vault.">
          <ImportPanel />
        </Panel>
      )}
    </div>
  );
};
