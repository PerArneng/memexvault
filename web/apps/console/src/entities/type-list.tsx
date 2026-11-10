import { entityPath, schemaView } from "@memex/core";
import type { EntityType, VaultId } from "@memex/types";
import { Badge, DataTable, EmptyState } from "@memex/ui";
import type { Column, Tone } from "@memex/ui";
import { useQuery } from "@tanstack/react-query";
import { useMemo } from "react";
import { useEngine } from "../engine-context";
import { unwrap } from "../query";
import { QueryView } from "../shell/query-view";
import { AppLink, Callout, formatTime, LINK_BUTTON } from "../wiki/wiki-common";
import { ENTITY_KEYS, SchemaProblems, typeLink, useEntityType } from "./entity-common";

const ROW_HEIGHT = 30;
const HEADER = 32;
const MIN_HEIGHT = 160;
const MAX_HEIGHT = 560;

const CHANGE_TONE: Readonly<Record<string, Tone>> = { breaking: "warning", additive: "success", new: "info", seed: "neutral" };
const ChangeBadge = ({ change }: { readonly change: string }) => <Badge tone={CHANGE_TONE[change] ?? "neutral"}>{change}</Badge>;

export const TypeList = ({ vault, writable }: { readonly vault: VaultId; readonly writable: boolean }) => {
  const engine = useEngine();
  const query = useQuery({ queryKey: ENTITY_KEYS.types(vault), queryFn: async () => unwrap(await engine.listEntityTypes(vault)) });
  const columns = useMemo((): readonly Column<EntityType>[] => [
    { id: "name", header: "Type", width: "minmax(0, 2fr)", cell: (t) => <AppLink to={typeLink(vault, t.name)}>{t.name}</AppLink> },
    { id: "version", header: "Version", width: "5rem", numeric: true, cell: (t) => t.version },
    { id: "fields", header: "Fields", width: "5rem", numeric: true, cell: (t) => schemaView(t.schema).fields.length },
    { id: "change", header: "Last change", width: "8rem", cell: (t) => <ChangeBadge change={t.change} /> },
    { id: "schema", header: "Schema", width: "6rem", cell: (t) => <AppLink to={entityPath(vault, { kind: "type", type: t.name })}>Versions</AppLink> },
  ], [vault]);
  return (
    <section aria-label="Entity types" className="flex flex-col gap-3">
      <div className="flex items-center justify-between">
        <h2 className="text-lg font-medium">Types</h2>
        {writable && <AppLink to={entityPath(vault, { kind: "new_type" })} className={LINK_BUTTON}>New type</AppLink>}
      </div>
      <QueryView query={query} label="entity types" isEmpty={(l) => l.types.length === 0} empty={<EmptyState title="This vault has no entity types." />}>
        {(list) => (
          <>
            {list.truncated && <Callout tone="warning" title={`Showing the first ${String(list.types.length)} types.`} />}
            <DataTable label="Entity types" rows={list.types} columns={columns} rowKey={(t) => t.name} height={Math.max(MIN_HEIGHT, Math.min(MAX_HEIGHT, HEADER + list.types.length * ROW_HEIGHT))} />
          </>
        )}
      </QueryView>
    </section>
  );
};

export const TypeDetail = ({ vault, type, writable }: { readonly vault: VaultId; readonly type: string; readonly writable: boolean }) => {
  const { query, view } = useEntityType(vault, type);
  return (
    <section aria-label={`Type ${type}`} className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h2 className="text-lg font-medium break-words">{type}</h2>
        <span className="flex gap-2">
          <AppLink to={typeLink(vault, type)} className={LINK_BUTTON}>Entities</AppLink>
          {writable && <AppLink to={entityPath(vault, { kind: "edit_type", type })} className={LINK_BUTTON}>Change this type</AppLink>}
        </span>
      </div>
      <QueryView query={query} label="this type">
        {(versions) => (
          <>
            {view !== null && <SchemaProblems problems={view.problems} />}
            {view !== null && (
              <table className="w-full text-sm" aria-label="Fields">
                <thead>
                  <tr className="border-b border-border-strong text-left text-xs text-text-muted">
                    <th className="p-1.5 font-medium">Field</th><th className="p-1.5 font-medium">Holds</th><th className="p-1.5 font-medium">Annotations</th><th className="p-1.5 font-medium">Description</th>
                  </tr>
                </thead>
                <tbody>
                  {view.fields.map((f) => (
                    <tr key={f.name} className="border-b border-border" data-schema-field={f.name}>
                      <td className="p-1.5 font-mono text-xs">{f.name}{f.required ? " *" : ""}</td>
                      <td className="p-1.5 text-xs">{f.isArray ? "list of " : ""}{f.kind}{f.format === null ? "" : ` (${f.format})`}</td>
                      <td className="p-1.5"><span className="flex flex-wrap gap-1">{f.identity && <Badge tone="info">identity</Badge>}{f.searchable && <Badge>searchable</Badge>}{f.filterable && <Badge>filterable</Badge>}</span></td>
                      <td className="p-1.5 text-xs text-text-muted">{f.description}</td>
                    </tr>
                  ))}
                </tbody>
              </table>
            )}
            <h3 className="font-medium">Versions</h3>
            <ol className="flex flex-col gap-2" aria-label="Versions">
              {[...versions].reverse().map((v) => (
                <li key={v.version} data-type-version={v.version} className="flex flex-col gap-1 rounded-md border border-border p-2 text-sm">
                  <span className="flex flex-wrap items-center gap-2">
                    <span className="font-medium">Version {v.version}</span> <ChangeBadge change={v.change} />
                    {v.createdAt > 0 && <span className="font-mono text-xs text-text-muted">{formatTime(v.createdAt)}</span>}
                  </span>
                  {v.reasons.length > 0 && <ul className="list-disc pl-4 text-xs text-text-muted">{v.reasons.map((r) => <li key={r}>{r}</li>)}</ul>}
                  <details className="text-xs">
                    <summary className="cursor-pointer text-text-muted">Its JSON Schema</summary>
                    <pre className="mt-1 max-h-64 overflow-auto rounded-sm bg-surface-raised p-2 font-mono whitespace-pre-wrap">{JSON.stringify(v.schema, null, 2)}</pre>
                  </details>
                </li>
              ))}
            </ol>
          </>
        )}
      </QueryView>
    </section>
  );
};
