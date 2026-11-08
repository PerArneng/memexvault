import { boardRows, canChangeSource, durationText, framesPerSecond, ingestPath, parseIngestRoute, progressText, stageCounts, stageLabel, throughputPerMinute } from "@memex/core";
import type { ActivityWatch, BoardDocument, BoardFilter } from "@memex/core";
import type { Identity, VaultId } from "@memex/types";
import { DataTable, EmptyState, Input, Skeleton } from "@memex/ui";
import type { Column } from "@memex/ui";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useEffect, useMemo, useRef, useState } from "react";
import { useEngine } from "../engine-context";
import { QUERY_KEYS, unwrap } from "../query";
import { AppLink, LINK_BUTTON, useGo } from "../wiki/wiki-common";
import { ConnectionBadge, connectionDetail, ProgressBar, StatusOf, useActivityState } from "./ingest-common";
import { UploadPanel } from "./upload-panel";

const FILTER_PAUSE_MS = 300;
const TABLE_HEIGHT = 480;

/** Names aren't in frames, by design, so they come from the sources list and follow when a document the list doesn't know shows up. */
const useSourceNames = (vault: VaultId, ids: Iterable<string>) => {
  const engine = useEngine();
  const queryClient = useQueryClient();
  const query = useQuery({ queryKey: QUERY_KEYS.sources(vault), queryFn: async () => unwrap(await engine.listSources(vault)), staleTime: 5000 });
  const names = useMemo(() => new Map((query.data?.sources ?? []).map((s) => [s.id, s.name])), [query.data]);
  // Each unknown id asks once, so one that the list will never hold can't keep asking.
  const asked = useRef(new Set<string>());
  const unknown = query.data === undefined || query.data.truncated ? [] : [...ids].filter((id) => !names.has(id) && !asked.current.has(id));
  const fetching = query.isFetching;
  useEffect(() => {
    if (unknown.length === 0 || fetching) return;
    for (const id of unknown) asked.current.add(id);
    void queryClient.invalidateQueries({ queryKey: QUERY_KEYS.sources(vault), refetchType: "active" });
  }, [unknown.length, fetching, queryClient, vault]);
  return names;
};

const StageFilters = ({ vault, counts, filter }: { readonly vault: VaultId; readonly counts: ReturnType<typeof stageCounts>; readonly filter: BoardFilter }) => (
  <ul aria-label="Documents per stage" className="grid grid-cols-5 gap-2 xl:grid-cols-10">
    {counts.map(({ stage, count, failed }) => {
      const on = filter.stage === stage;
      return (
        <li key={stage} data-stage-count={stage} data-count={count}>
          <AppLink
            to={ingestPath(vault, { kind: "board", stage: on ? "" : stage, status: filter.status, text: filter.text })}
            aria-current={on ? "page" : undefined}
            className={`flex flex-col rounded-md border px-2 py-1 ${on ? "border-accent bg-surface-selected" : "border-border bg-surface-raised hover:bg-surface-overlay"}`}
          >
            <span className="truncate text-xs text-text-muted">{stageLabel(stage)}</span>
            <span className="font-mono text-lg tabular-nums">{count}</span>
            {failed > 0 && <span className="text-xs text-danger">{failed} failed</span>}
          </AppLink>
        </li>
      );
    })}
  </ul>
);

const Row = ({ doc }: { readonly doc: BoardDocument }) => {
  const text = progressText(doc.progress);
  return (
    <span data-source-row={doc.sourceId} data-stage={doc.stage} data-status={doc.status} data-done={doc.progress?.done ?? ""} className="flex min-w-0 flex-col justify-center">
      {text === null ? <span className="text-text-muted">—</span> : <span className="truncate text-xs">{text}</span>}
      {doc.progress?.total !== null && doc.progress !== null && doc.progress.total !== undefined && <ProgressBar label={text ?? "Progress"} value={doc.progress.done} max={doc.progress.total} />}
    </span>
  );
};

export const LiveBoard = ({ identity, vault, watch, rest, search }: { readonly identity: Identity; readonly vault: VaultId; readonly watch: ActivityWatch; readonly rest: readonly string[]; readonly search: string }) => {
  const { state, now } = useActivityState(watch);
  const go = useGo();
  const query = new URLSearchParams(search);
  const route = parseIngestRoute(rest, (name) => query.get(name));
  const filter: BoardFilter = route.kind === "board" ? route : { stage: "", status: "", text: "" };
  const names = useSourceNames(vault, state.documents.keys());
  const failure = watch.failure();
  const canUpload = canChangeSource(identity, vault);

  // Typing stays local, and the address follows after a pause, so a keystroke never waits for the router.
  const [text, setText] = useState(filter.text);
  const typed = useRef(filter.text);
  useEffect(() => {
    if (text === typed.current) return;
    const timer = setTimeout(() => {
      typed.current = text;
      go(ingestPath(vault, { kind: "board", stage: filter.stage, status: filter.status, text }), true);
    }, FILTER_PAUSE_MS);
    return () => clearTimeout(timer);
  }, [filter.stage, filter.status, go, text, vault]);

  const counts = useMemo(() => stageCounts(state.documents.values()), [state.documents]);
  const rows = useMemo(() => boardRows(state.documents.values(), { ...filter, text }, (id) => names.get(id)), [state.documents, filter.stage, filter.status, text, names]);
  const failedCount = useMemo(() => [...state.documents.values()].filter((d) => d.status === "failed").length, [state.documents]);
  const detail = connectionDetail(state.connection);

  const columns: readonly Column<BoardDocument>[] = [
    { id: "name", header: "Document", width: "minmax(12rem, 2fr)", cell: (d) => <AppLink to={ingestPath(vault, { kind: "document", id: d.sourceId })} className="block truncate text-accent underline">{names.get(d.sourceId) ?? d.sourceId}</AppLink> },
    { id: "stage", header: "Stage", width: "7rem", cell: (d) => stageLabel(d.stage) },
    { id: "status", header: "Status", width: "8rem", cell: (d) => <StatusOf doc={d} /> },
    { id: "progress", header: "Progress", width: "minmax(10rem, 1fr)", cell: (d) => <Row doc={d} /> },
    { id: "time", header: "In stage", width: "7rem", numeric: true, cell: (d) => (d.stage === "done" ? "—" : durationText(now - d.stageSinceMs)) },
    { id: "attempts", header: "Attempts", width: "5rem", numeric: true, cell: (d) => String(d.attempts) },
  ];

  return (
    <div className="flex flex-col gap-4" data-applied-seq={state.seq ?? ""} data-document-count={state.documents.size}>
      {canUpload ? (
        <UploadPanel vault={vault} />
      ) : (
        <p data-no-upload={true} className="text-sm text-text-muted">Your token can read this vault, so you can watch what's happening here. Uploading, retrying and cancelling need <strong>propose</strong> scope.</p>
      )}
      <div className="flex flex-wrap items-center gap-4 text-sm">
        <ConnectionBadge connection={state.connection} />
        <span data-throughput={true} className="text-text-muted"><span className="font-mono tabular-nums text-text">{throughputPerMinute(state, now).toFixed(1)}</span> finished per minute</span>
        <span className="text-text-muted"><span className="font-mono tabular-nums text-text">{framesPerSecond(state, now).toFixed(1)}</span> events per second</span>
        <AppLink to={ingestPath(vault, { kind: "failed", state: "dead" })} className={LINK_BUTTON}>Failed jobs</AppLink>
        {failedCount > 0 && (
          <AppLink to={ingestPath(vault, { kind: "board", stage: "", status: filter.status === "failed" ? "" : "failed", text: filter.text })} aria-current={filter.status === "failed" ? "page" : undefined} className="text-danger underline">
            {filter.status === "failed" ? "Show all" : `Show ${String(failedCount)} failed`}
          </AppLink>
        )}
      </div>
      {detail !== null && <p role="alert" data-connection-detail={true} className="rounded-md border border-warning bg-warning-surface p-2 text-sm text-warning">{detail}</p>}
      {state.truncated && <p className="text-sm text-warning">This vault has more documents than one browser list holds, so this shows the first 10,000, and the counts cover those.</p>}
      <StageFilters vault={vault} counts={counts} filter={filter} />
      <Input label="Filter by name or id" value={text} onChange={(e) => setText(e.target.value)} />
      {state.seq === null ? (
        failure === null ? (
          <div className="flex flex-col gap-2"><p role="status" className="sr-only">Loading the live board</p><Skeleton className="h-24 w-full" /></div>
        ) : (
          <EmptyState title="Couldn't load the live board.">{failure.message} It tries again by itself.</EmptyState>
        )
      ) : state.documents.size === 0 ? (
        <EmptyState title="There's nothing in this vault yet.">{canUpload ? "Upload a document or a recording above, and it shows here as it moves through the pipeline." : "When someone uploads a document or a recording, it shows here as it moves through the pipeline."}</EmptyState>
      ) : rows.length === 0 ? (
        <EmptyState title="No documents match this filter." />
      ) : (
        <DataTable label="Documents" rows={rows} columns={columns} rowKey={(d) => d.sourceId} height={TABLE_HEIGHT} />
      )}
    </div>
  );
};
