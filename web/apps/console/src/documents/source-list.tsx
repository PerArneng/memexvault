import { canChangeSource, filterSources, flaggedSources, formatSize, mediaKind, sourceKinds, TRUST_TIERS, documentPath } from "@memex/core";
import type { SourceFilter } from "@memex/core";
import type { DocumentSource, Identity, VaultId } from "@memex/types";
import { Badge, Button, DataTable, EmptyState, Input } from "@memex/ui";
import type { Column } from "@memex/ui";
import { useQuery } from "@tanstack/react-query";
import { useMemo } from "react";
import { useEngine } from "../engine-context";
import { QUERY_KEYS, unwrap } from "../query";
import { QueryView } from "../shell/query-view";
import { TierOf } from "../viewer/badges";
import { AppLink, formatTime, useGo } from "../wiki/wiki-common";
import { useSourcePath } from "./document-common";

const TABLE_ROW_HEIGHT = 30;
const MIN_TABLE_HEIGHT = 240;
const MAX_TABLE_HEIGHT = 600;
const TABLE_HEADER = 32;

const KIND_LABELS: Readonly<Record<string, string>> = { pdf: "PDF", text: "Text", audio: "Audio", video: "Video", other: "Other" };

export const SourceList = ({ identity, vault, filter }: { readonly identity: Identity; readonly vault: VaultId; readonly filter: SourceFilter }) => {
  const engine = useEngine();
  const go = useGo();
  const pathOf = useSourcePath(vault);
  const query = useQuery({ queryKey: QUERY_KEYS.sources(vault), queryFn: async () => unwrap(await engine.listSources(vault)) });
  // Only when it's asked for, since it reads every flagged page in the vault.
  const flaggedQuery = useQuery({ queryKey: QUERY_KEYS.flaggedPages(vault), queryFn: async () => unwrap(await engine.listFlaggedPages(vault)), enabled: filter.flagged });
  const flagged = useMemo(() => flaggedSources(flaggedQuery.data ?? []), [flaggedQuery.data]);
  const setFilter = (over: Partial<SourceFilter>) => go(documentPath(vault, { kind: "list", filter: { ...filter, ...over } }), true);

  const columns = useMemo((): readonly Column<DocumentSource>[] => [
    { id: "name", header: "Name", width: "minmax(0, 3fr)", cell: (s) => <span className="block truncate"><AppLink to={pathOf(s.id)}>{s.name}</AppLink></span> },
    { id: "kind", header: "Kind", width: "5rem", cell: (s) => <span className="text-xs text-text-muted">{KIND_LABELS[mediaKind(s.mediaType)]}</span> },
    { id: "trust", header: "Trust tier", width: "8rem", cell: (s) => <TierOf tier={s.trustTier} /> },
    { id: "version", header: "Version", width: "5rem", numeric: true, cell: (s) => s.versionNo },
    { id: "size", header: "Size", width: "6rem", numeric: true, cell: (s) => <span className="text-xs text-text-muted tabular-nums">{formatSize(s.size)}</span> },
    { id: "recorded", header: "Recorded", width: "11rem", cell: (s) => <span className="font-mono text-xs text-text-muted tabular-nums">{formatTime(s.recordedAt)}</span> },
  ], [pathOf]);

  return (
    <section aria-label="All documents" className="flex flex-col gap-3">
      <div className="flex flex-wrap items-end gap-2">
        <div className="w-64">
          <Input label="Filter by name" value={filter.text} onChange={(e) => setFilter({ text: e.target.value })} />
        </div>
        <label className="flex flex-col gap-1 text-xs text-text-muted">
          Trust tier
          <select value={filter.tier} onChange={(e) => setFilter({ tier: e.target.value })} className="h-8 rounded-md border border-border-strong bg-surface-raised px-1 text-sm text-text">
            <option value="">Any</option>
            {TRUST_TIERS.map((tier) => <option key={tier} value={tier}>{tier}</option>)}
          </select>
        </label>
        <label className="flex flex-col gap-1 text-xs text-text-muted">
          Kind
          <select value={filter.kind} onChange={(e) => setFilter({ kind: e.target.value })} className="h-8 rounded-md border border-border-strong bg-surface-raised px-1 text-sm text-text">
            <option value="">Any</option>
            {sourceKinds.map((kind) => <option key={kind.id} value={kind.id}>{kind.label}</option>)}
          </select>
        </label>
        <Button aria-pressed={filter.flagged} onClick={() => setFilter({ flagged: !filter.flagged })}>Flagged pages</Button>
      </div>
      <QueryView
        query={query}
        label="documents"
        isEmpty={(data) => data.sources.length === 0}
        empty={
          <EmptyState title="There are no documents in this vault yet.">
            {canChangeSource(identity, vault) ? "Upload one through the API, as the console's upload screen comes with Ingest." : "Someone with propose scope can add them."}
          </EmptyState>
        }
      >
        {({ sources, truncated }) => {
          const rows = filterSources(sources, filter, flagged);
          return (
            <>
              <p className="flex items-center gap-2 text-xs text-text-muted">
                {rows.length} of {sources.length} documents
                {truncated && <Badge tone="warning">Only the first {sources.length} are listed, so filters only see those.</Badge>}
              </p>
              {rows.length === 0 ? (
                <EmptyState title="No documents match these filters." />
              ) : (
                <DataTable label="Documents" rows={rows} columns={columns} rowKey={(s) => s.id} height={Math.max(MIN_TABLE_HEIGHT, Math.min(MAX_TABLE_HEIGHT, rows.length * TABLE_ROW_HEIGHT + TABLE_HEADER))} onRowOpen={(s) => go(pathOf(s.id))} />
              )}
            </>
          );
        }}
      </QueryView>
    </section>
  );
};
