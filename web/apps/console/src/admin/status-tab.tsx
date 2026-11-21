import { formatBytes, healthLines, hitRate, statusFailureText } from "@memex/core";
import type { SystemStatus } from "@memex/types";
import { Badge, Button, Skeleton } from "@memex/ui";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { useEngine } from "../engine-context";
import { FailureError, QUERY_KEYS, unwrap } from "../query";
import { Callout, ConfirmPanel, KeyValues, Panel } from "./admin-common";
import { ReportView } from "./operations-tab";

/** Every 2 s, which is what spec 039's status promises to answer within. */
export const STATUS_INTERVAL_MS = 2000;

const TONE = { success: "success", warning: "warning", danger: "danger", neutral: "neutral" } as const;

const Health = ({ status }: { readonly status: SystemStatus }) => (
  <ul aria-label="Health" className="flex flex-col gap-2">
    {healthLines(status).map((line) => (
      <li key={line.id} data-health={line.id} data-tone={line.tone} className="flex flex-col gap-0.5">
        <span className="inline-flex items-center gap-2"><Badge tone={TONE[line.tone]}>{line.tone === "success" ? "ok" : line.tone === "danger" ? "down" : line.tone === "warning" ? "check" : "off"}</Badge> <span className="font-medium">{line.title}</span></span>
        {line.detail !== "" && <span className="text-sm text-text-muted">{line.detail}</span>}
      </li>
    ))}
  </ul>
);

const Vaults = ({ status }: { readonly status: SystemStatus }) =>
  status.vaults === null ? (
    <p className="text-sm text-text-muted">Not available: {status.vaultsError ?? "they couldn't be read in time."}</p>
  ) : (
    <table className="w-full text-left text-sm">
      <thead className="text-xs text-text-muted">
        <tr><th className="py-1 pr-3 font-normal">Vault</th><th className="pr-3 font-normal">Head</th><th className="pr-3 font-normal">Keyword lag</th><th className="pr-3 font-normal">Vector lag</th><th className="pr-3 font-normal">Open jobs</th><th className="pr-3 font-normal">Dead jobs</th><th className="pr-3 font-normal">Review</th><th className="font-normal">Files</th></tr>
      </thead>
      <tbody>
        {status.vaults.map((v) => (
          <tr key={v.id} data-vault-status={v.id} className="border-t border-border tabular-nums">
            <td className="py-1.5 pr-3">{v.name}</td>
            <td className="pr-3">{v.head ?? "–"}</td>
            <td className="pr-3">{v.keyword.lag}{v.keyword.building && " (building)"}{!v.keyword.ready && " (not ready)"}</td>
            <td className="pr-3">{v.vector.lag}{v.vector.building && " (building)"}{!v.vector.ready && " (not ready)"}</td>
            <td className="pr-3">{v.openJobs.length === 0 ? 0 : v.openJobs.map((j) => `${j.kind} ${String(j.count)}`).join(", ")}</td>
            <td className="pr-3">{v.deadJobs}</td>
            <td className="pr-3">{v.reviewPending}</td>
            <td>{v.blobs} ({formatBytes(v.blobBytes)})</td>
          </tr>
        ))}
      </tbody>
    </table>
  );

const Caches = ({ status }: { readonly status: SystemStatus }) => {
  const engine = useEngine();
  const queryClient = useQueryClient();
  const [confirming, setConfirming] = useState(false);
  const [cleared, setCleared] = useState<string | null>(null);
  return (
    <div className="flex flex-col gap-2">
      <table className="w-full text-left text-sm">
        <thead className="text-xs text-text-muted"><tr><th className="py-1 pr-3 font-normal">Tier</th><th className="pr-3 font-normal">Size</th><th className="pr-3 font-normal">Entries</th><th className="pr-3 font-normal">Hit rate</th><th className="pr-3 font-normal">Evictions</th><th className="font-normal">Rejected</th></tr></thead>
        <tbody>
          {status.caches.map((c) => (
            <tr key={c.tier} className="border-t border-border tabular-nums">
              <td className="py-1.5 pr-3">{c.tier}</td>
              <td className="pr-3">{c.capBytes === 0 ? "off" : `${formatBytes(c.sizeBytes)} of ${formatBytes(c.capBytes)}`}</td>
              <td className="pr-3">{c.entries}</td>
              <td className="pr-3">{hitRate(c.hits, c.misses)}</td>
              <td className="pr-3">{c.evictions}</td>
              <td>{c.rejected}{c.corrupt > 0 && `, ${String(c.corrupt)} corrupt`}</td>
            </tr>
          ))}
        </tbody>
      </table>
      {cleared !== null && <Callout tone="info" title={cleared} />}
      {confirming ? (
        <ConfirmPanel
          title="Clear every cache"
          description="Everything cached is dropped in every vault, so the next searches and reads are slower until the caches fill again. Nothing stored is lost."
          confirmLabel="Clear caches"
          run={async () => {
            const result = await engine.clearCaches();
            if (!result.ok) return result.failure;
            setCleared(`Cleared the caches of ${String(result.value.vaults)} vaults.`);
            await queryClient.invalidateQueries({ queryKey: QUERY_KEYS.systemStatus });
            return null;
          }}
          onDone={() => setConfirming(false)}
          onCancel={() => setConfirming(false)}
        />
      ) : (
        <div><Button onClick={() => setConfirming(true)}>Clear caches…</Button></div>
      )}
    </div>
  );
};

/**
 * Reads the status every 2 s. Spec 039's route answers within 2 s with Postgres or the LLM down, and while Postgres is down it still
 * knows a server admin it checked in the last 5 minutes, so this page keeps explaining what's wrong when the rest of the console can't.
 */
export const StatusTab = () => {
  const engine = useEngine();
  const query = useQuery({
    queryKey: QUERY_KEYS.systemStatus,
    queryFn: async () => unwrap(await engine.getSystemStatus()),
    refetchInterval: STATUS_INTERVAL_MS,
    // This page says what failed in place, so a toast every 2 s would only repeat it.
    meta: { ownsErrors: true },
  });
  const failure = query.error instanceof FailureError ? query.error.failure : null;
  return (
    <div className="flex flex-col gap-4">
      <p className="text-xs text-text-muted" role="status" data-testid="status-updated">
        {query.dataUpdatedAt === 0 ? "Reading the status…" : `Updated at ${new Date(query.dataUpdatedAt).toISOString().slice(11, 19)} UTC. It reads again every 2 seconds.`}
      </p>
      {failure !== null && (
        <Callout tone="danger" title={query.data === undefined ? "Couldn't read the server's status." : "The latest status read failed, so what follows may be out of date."}>
          <span data-testid="status-failure">{statusFailureText(failure)}</span>
        </Callout>
      )}
      {query.data === undefined ? (
        failure === null && <Skeleton className="h-24 w-full" />
      ) : (
        <>
          <Panel title="Health"><Health status={query.data} /></Panel>
          <Panel title="Server">
            <KeyValues
              label="The server"
              rows={[
                ["Version", query.data.version],
                ["Memory", query.data.memoryRssBytes === null ? "not reported on this platform" : formatBytes(query.data.memoryRssBytes)],
                ["Postgres", query.data.postgres.reachable ? `reachable, ${String(query.data.postgres.latencyMs ?? 0)} ms` : `down: ${query.data.postgres.error ?? "no answer"}`],
                ["LLM", !query.data.llm.configured ? "not configured" : query.data.llm.reachable ? `reachable, ${String(query.data.llm.latencyMs ?? 0)} ms` : `down: ${query.data.llm.error ?? "no answer"}`],
              ]}
            />
            <table className="w-full max-w-lg text-left text-sm">
              <thead className="text-xs text-text-muted"><tr><th className="py-1 pr-3 font-normal">Model</th><th className="pr-3 font-normal">Name</th><th className="font-normal">Status</th></tr></thead>
              <tbody>{query.data.models.map((m) => <tr key={m.role} className="border-t border-border"><td className="py-1 pr-3">{m.role}</td><td className="pr-3 font-mono text-xs">{m.model ?? "–"}</td><td>{m.status}</td></tr>)}</tbody>
            </table>
          </Panel>
          <Panel title="Vaults" description="Each vault's last event, how far each index is behind it, its queues and its stored files."><Vaults status={query.data} /></Panel>
          <Panel title="Caches"><Caches status={query.data} /></Panel>
          <Panel title="Backends and thresholds">
            <ReportView report={{ backends: query.data.backends, thresholds: query.data.thresholds }} label="Backends and thresholds" />
          </Panel>
        </>
      )}
    </div>
  );
};
