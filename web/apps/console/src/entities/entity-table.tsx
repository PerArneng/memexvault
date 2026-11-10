import { attributeOf, entityFilterText, entityLabel, entityPath, FILTER_OP_LABELS, filterProblem, requestIdText, sortLoadedRows, valueText } from "@memex/core";
import type { EntityColumn, EntityFilter, EntityFilterDef, EntitySort, EntityTableRoute, FilterOp, SchemaField, SchemaView } from "@memex/core";
import type { EntitySummary, Page, VaultId } from "@memex/types";
import { Button, ConflictBadge, DataTable, EmptyState, Input, Badge } from "@memex/ui";
import type { Column } from "@memex/ui";
import { keepPreviousData, useInfiniteQuery, useQuery } from "@tanstack/react-query";
import type { InfiniteData, UseQueryResult } from "@tanstack/react-query";
import { useCallback, useMemo, useState } from "react";
import { useEngine } from "../engine-context";
import { FailureError, unwrap } from "../query";
import { QueryView } from "../shell/query-view";
import { AppLink, Callout, LINK_BUTTON, useGo } from "../wiki/wiki-common";
import { ENTITY_KEYS, FIELD_LABEL, SchemaProblems, SELECT_CLASS, useEntityType } from "./entity-common";

const TABLE_HEIGHT = 560;
/** A new entity is searchable within the indexing lag, about a second, so a search with no hits looks again for a few seconds. */
const FIND_RETRY_MS = 500;
const FIND_RETRIES = 10;

interface FilterRow {
  readonly key: number;
  readonly field: string;
  readonly op: FilterOp;
  readonly value: string;
}

const FilterValueInput = ({ def, id, value, onChange }: { readonly def: EntityFilterDef; readonly id: string; readonly value: string; readonly onChange: (next: string) => void }) => {
  if (def.input === "boolean" || def.input === "choice") {
    const options = def.input === "boolean" ? ["true", "false"] : def.options;
    return (
      <select id={id} className={SELECT_CLASS} value={value} onChange={(e) => onChange(e.target.value)}>
        <option value="">Choose</option>
        {options.map((o) => <option key={o} value={o}>{o}</option>)}
      </select>
    );
  }
  const type = def.input === "number" ? "number" : def.input === "date" ? "date" : "text";
  return <input id={id} type={type} step="any" className={SELECT_CLASS} placeholder={def.input === "date-time" ? "2026-01-31T12:00:00Z" : undefined} value={value} onChange={(e) => onChange(e.target.value)} />;
};

/**
 * Filters run on the server, across every row, and only when asked: an Apply rather than a request per keystroke, so typing never waits on anything and
 * the time a filter takes is the API's plus this screen's.
 */
const FilterBar = ({ defs, applied, onApply }: { readonly defs: readonly EntityFilterDef[]; readonly applied: readonly EntityFilter[]; readonly onApply: (filters: readonly EntityFilter[]) => void }) => {
  const [rows, setRows] = useState<readonly FilterRow[]>(() => applied.map((f, key) => ({ key, ...f })));
  const [nextKey, setNextKey] = useState(applied.length);
  const [problem, setProblem] = useState<string | null>(null);
  const first = defs[0];
  if (first === undefined) return <p className="text-sm text-text-muted">No field of this type is filterable.</p>;
  const defOf = (field: string) => defs.find((d) => d.field === field);
  const change = (key: number, over: Partial<FilterRow>) => setRows(rows.map((r) => (r.key === key ? { ...r, ...over } : r)));
  const apply = () => {
    for (const row of rows) {
      const def = defOf(row.field);
      const wrong = def === undefined ? `${row.field} can't be filtered on.` : filterProblem(def, row.op, row.value);
      if (wrong !== null) {
        setProblem(wrong);
        return;
      }
    }
    setProblem(null);
    onApply(rows.map(({ field, op, value }) => ({ field, op, value: value.trim() })));
  };
  return (
    <form aria-label="Filters" className="flex flex-col gap-2" onSubmit={(e) => { e.preventDefault(); apply(); }}>
      {rows.map((row) => {
        const def = defOf(row.field);
        const id = `filter-${String(row.key)}`;
        return (
          <div key={row.key} className="flex flex-wrap items-end gap-2" data-filter-row={row.field}>
            <div className="flex flex-col gap-1">
              <label htmlFor={`${id}-field`} className={FIELD_LABEL}>Field</label>
              <select id={`${id}-field`} className={SELECT_CLASS} value={row.field} onChange={(e) => change(row.key, { field: e.target.value, op: "eq", value: "" })}>
                {defs.map((d) => <option key={d.field} value={d.field}>{d.label}</option>)}
                {def === undefined && <option value={row.field}>{row.field}</option>}
              </select>
            </div>
            <div className="flex flex-col gap-1">
              <label htmlFor={`${id}-op`} className={FIELD_LABEL}>Compare</label>
              <select id={`${id}-op`} className={SELECT_CLASS} value={row.op} onChange={(e) => change(row.key, { op: e.target.value as FilterOp })}>
                {(def?.ops ?? [row.op]).map((op) => <option key={op} value={op}>{FILTER_OP_LABELS[op]}</option>)}
              </select>
            </div>
            <div className="flex flex-col gap-1">
              <label htmlFor={`${id}-value`} className={FIELD_LABEL}>Value</label>
              {def === undefined ? <input id={`${id}-value`} readOnly={true} className={SELECT_CLASS} value={row.value} /> : <FilterValueInput def={def} id={`${id}-value`} value={row.value} onChange={(value) => change(row.key, { value })} />}
            </div>
            <Button variant="ghost" onClick={() => setRows(rows.filter((r) => r.key !== row.key))}>Remove</Button>
          </div>
        );
      })}
      {problem !== null && <Callout tone="warning" title={problem} />}
      <div className="flex flex-wrap gap-2">
        <Button onClick={() => {
          setRows([...rows, { key: nextKey, field: first.field, op: first.ops[0] ?? "eq", value: "" }]);
          setNextKey(nextKey + 1);
        }}>Add a filter</Button>
        <Button type="submit" variant="primary">Apply filters</Button>
      </div>
    </form>
  );
};

const ColumnChooser = ({ columns, visible, onChange }: { readonly columns: readonly EntityColumn[]; readonly visible: readonly string[]; readonly onChange: (next: readonly string[]) => void }) => (
  <details className="relative text-sm" data-column-chooser={true}>
    <summary className="inline-flex h-8 cursor-pointer items-center rounded-md border border-border-strong bg-surface-overlay px-3 hover:bg-surface-selected">Columns ({visible.length} of {columns.length})</summary>
    <fieldset className="mt-1 grid max-h-64 grid-cols-2 gap-1 overflow-auto rounded-md border border-border-strong bg-surface-raised p-2">
      <legend className="sr-only">Columns to show</legend>
      {columns.map((c) => (
        <label key={c.id} className="flex items-center gap-1 text-xs">
          <input type="checkbox" checked={visible.includes(c.id)} onChange={(e) => onChange(e.target.checked ? columns.filter((x) => x.id === c.id || visible.includes(x.id)).map((x) => x.id) : visible.filter((v) => v !== c.id))} /> {c.label}
        </label>
      ))}
    </fieldset>
  </details>
);

const FindResults = ({ vault, type, mention, fields }: { readonly vault: VaultId; readonly type: string; readonly mention: string; readonly fields: readonly SchemaField[] }) => {
  const engine = useEngine();
  const query = useQuery({
    queryKey: ENTITY_KEYS.resolve(vault, type, mention),
    queryFn: async () => unwrap(await engine.resolveEntities(vault, mention, type)),
    staleTime: 0,
    refetchInterval: (q) => (q.state.data?.candidates.length === 0 && q.state.dataUpdateCount < FIND_RETRIES ? FIND_RETRY_MS : false),
  });
  return (
    <section aria-label="Search results" data-find={mention} className="rounded-md border border-border p-2">
      <QueryView query={query} label="the search" isEmpty={(r) => r.candidates.length === 0} empty={<p className="text-sm text-text-muted" data-find-empty={true}>Nothing of this type matches. A value saved a moment ago can take about a second to be searchable.</p>}>
        {(found) => (
          <>
            {found.degraded.length > 0 && <Callout tone="warning" title="Keyword search can't be reached, so only exact matches are shown." />}
            <ul className="flex flex-col gap-1 text-sm">
              {found.candidates.map((c) => (
                <li key={c.entity.id} data-find-hit={c.entity.id} className="flex flex-wrap items-center gap-2">
                  <AppLink to={entityPath(vault, { kind: "entity", id: c.entity.id })}>{entityLabel(c.entity.attributes, c.entity.aliases, fields) ?? c.entity.id}</AppLink>
                  {c.exact ? <Badge tone="success">Exact: {c.matchedOn === "$alias" ? "alias" : c.matchedOn}</Badge> : <Badge>Keyword</Badge>}
                </li>
              ))}
            </ul>
          </>
        )}
      </QueryView>
    </section>
  );
};

const SORT_MARK: Readonly<Record<"asc" | "desc", string>> = { asc: "↑", desc: "↓" };

const nextSort = (field: string, sort: EntitySort | null): EntitySort | null => {
  if (sort?.field !== field) return { field, direction: "asc" };
  return sort.direction === "asc" ? { field, direction: "desc" } : null;
};

const Rows = ({ vault, route, view }: { readonly vault: VaultId; readonly route: EntityTableRoute; readonly view: SchemaView }) => {
  const engine = useEngine();
  const go = useGo();
  const filterKey = route.filters.map(entityFilterText).join("&");
  const query = useInfiniteQuery({
    queryKey: ENTITY_KEYS.rows(vault, route.type, filterKey),
    queryFn: async ({ pageParam }) => unwrap(await engine.listEntities(vault, route.type, route.filters, pageParam)),
    initialPageParam: null as string | null,
    getNextPageParam: (last: Page<EntitySummary>) => last.nextCursor,
    placeholderData: keepPreviousData,
  });
  const rows = useMemo(() => query.data?.pages.flatMap((p) => p.items) ?? [], [query.data]);
  const known = useMemo(() => new Map(view.columns.map((c) => [c.id, c])), [view.columns]);
  const visibleIds = useMemo(() => route.columns?.filter((c) => known.has(c)) ?? view.columns.filter((c) => c.visibleByDefault).map((c) => c.id), [known, route.columns, view.columns]);
  const sortColumn = route.sort === null ? undefined : known.get(route.sort.field);
  const sort = sortColumn === undefined ? null : route.sort;
  const sorted = useMemo(() => (sort === null ? rows : sortLoadedRows(rows, (r) => attributeOf(r.attributes, sort.field), sortColumn?.kind ?? "text", sort.direction)), [rows, sort, sortColumn?.kind]);
  const to = useCallback((over: Partial<EntityTableRoute>) => go(entityPath(vault, { ...route, ...over }), true), [go, route, vault]);

  const columns = useMemo((): readonly Column<EntitySummary>[] => {
    const chosen = visibleIds.flatMap((id) => known.get(id) ?? []);
    const [lead] = chosen;
    return [
      ...chosen.map((c): Column<EntitySummary> => {
        const sortedHere = sort?.field === c.id ? sort.direction : null;
        return {
          id: c.id,
          header: c.label,
          width: c.width,
          numeric: c.numeric,
          ariaSort: sortedHere === null ? "none" : sortedHere === "asc" ? "ascending" : "descending",
          // Sorting says what it covers, as the API only lists by id: see the note this turns on.
          headerCell: (
            <button type="button" className="inline-flex items-center gap-1 hover:text-text" title="Sort the rows loaded so far" onClick={() => to({ sort: nextSort(c.id, sort) })}>
              {c.label} {sortedHere !== null && <span aria-hidden={true}>{SORT_MARK[sortedHere]}</span>}
            </button>
          ),
          cell: (row) => {
            const text = valueText(attributeOf(row.attributes, c.id));
            return c === lead ? <AppLink to={entityPath(vault, { kind: "entity", id: row.id })}>{text === "" ? row.id : text}</AppLink> : <span title={text}>{text}</span>;
          },
        };
      }),
      { id: "$conflicts", header: "Conflicts", width: "7rem", cell: (row) => (row.conflictedFields.length > 0 ? <ConflictBadge count={row.conflictedFields.length} /> : null) },
      ...(chosen.length === 0 ? [{ id: "$id", header: "Entity", width: "minmax(0, 1fr)", cell: (row: EntitySummary) => <AppLink to={entityPath(vault, { kind: "entity", id: row.id })}>{row.id}</AppLink> }] : []),
    ];
  }, [known, sort, to, vault, visibleIds]);

  const { hasNextPage, isFetchingNextPage, fetchNextPage, isFetchNextPageError, isPlaceholderData } = query;
  const loadMore = useCallback(() => {
    if (hasNextPage && !isFetchingNextPage && !isFetchNextPageError) void fetchNextPage();
  }, [fetchNextPage, hasNextPage, isFetchNextPageError, isFetchingNextPage]);
  const failure = query.error instanceof FailureError ? query.error.failure : null;

  return (
    <div className="flex flex-col gap-2">
      <div className="flex flex-wrap items-start gap-2">
        <ColumnChooser columns={view.columns} visible={visibleIds} onChange={(next) => to({ columns: next })} />
        <p role="status" className="py-1.5 text-sm text-text-muted" data-entity-count={rows.length} data-filter-key={filterKey} data-settled={!isPlaceholderData && !query.isPending}>
          {isPlaceholderData ? "Filtering…" : `${rows.length.toLocaleString("en")} ${rows.length === 1 ? "row" : "rows"} loaded${hasNextPage ? ", more as you scroll" : ", which is all of them"}`}
        </p>
      </div>
      {sort !== null && (
        <div data-sort-note={true}>
          <Callout tone="warning" title={`Sorted by ${sort.field} within the ${rows.length.toLocaleString("en")} rows loaded so far, not across every row.`}>
            The server lists entities by id, so a row that isn't loaded yet can belong anywhere in this order. Filters do cover every row.
            {" "}Loading stops while sorted, so rows don't move under you.
            {hasNextPage && <> <Button onClick={loadMore} disabled={isFetchingNextPage}>Load more rows</Button></>}
            {" "}<Button variant="ghost" onClick={() => to({ sort: null })}>Clear the sort</Button>
          </Callout>
        </div>
      )}
      {isFetchNextPageError && (
        <Callout tone="danger" title={`Couldn't load more rows: ${query.error?.message ?? "no answer"}`}>
          {failure === null ? null : requestIdText(failure)} <Button onClick={() => void fetchNextPage()}>Try again</Button>
        </Callout>
      )}
      <QueryView
        query={query as unknown as UseQueryResult<InfiniteData<Page<EntitySummary>>>}
        label="these entities"
        isEmpty={(d) => d.pages.every((p) => p.items.length === 0)}
        empty={<EmptyState title={route.filters.length > 0 ? "No entity matches these filters." : "No entities of this type yet."} />}
      >
        {() => <DataTable label={`${route.type} entities`} rows={sorted} columns={columns} rowKey={(r) => r.id} height={TABLE_HEIGHT} {...(sort === null ? { onNearEnd: loadMore } : {})} />}
      </QueryView>
    </div>
  );
};

/** A table generated from the type's schema, so a new type needs nothing added here. */
export const EntityTable = ({ vault, route, writable }: { readonly vault: VaultId; readonly route: EntityTableRoute; readonly writable: boolean }) => {
  const go = useGo();
  const { query, view } = useEntityType(vault, route.type);
  const [mention, setMention] = useState(route.find);
  const to = (over: Partial<EntityTableRoute>) => go(entityPath(vault, { ...route, ...over }), true);
  const filterKey = route.filters.map(entityFilterText).join("&");
  return (
    <section aria-label={`${route.type} entities`} className="flex flex-col gap-3">
      <div className="flex flex-wrap items-center justify-between gap-2">
        <h2 className="text-lg font-medium break-words">{route.type}</h2>
        <span className="flex flex-wrap gap-2">
          <AppLink to={entityPath(vault, { kind: "types" })} className={LINK_BUTTON}>All types</AppLink>
          <AppLink to={entityPath(vault, { kind: "type", type: route.type })} className={LINK_BUTTON}>Schema and versions</AppLink>
          <AppLink to={entityPath(vault, { kind: "same_as", type: route.type })} className={LINK_BUTTON}>Same-as suggestions</AppLink>
          {writable && <AppLink to={entityPath(vault, { kind: "new_entity", type: route.type })} className={LINK_BUTTON}>New entity</AppLink>}
        </span>
      </div>
      <QueryView query={query} label="this type">
        {() =>
          view === null ? null : (
            <>
              <SchemaProblems problems={view.problems} />
              <form aria-label="Find an entity" className="flex items-end gap-2" onSubmit={(e) => { e.preventDefault(); to({ find: mention.trim() }); }}>
                <div className="w-72"><Input label="Find by name or keyword" value={mention} onChange={(e) => setMention(e.target.value)} /></div>
                <Button type="submit">Find</Button>
                {route.find !== "" && <Button variant="ghost" onClick={() => { setMention(""); to({ find: "" }); }}>Clear</Button>}
              </form>
              {route.find !== "" && <FindResults vault={vault} type={route.type} mention={route.find} fields={view.fields} />}
              <FilterBar key={filterKey} defs={view.filters} applied={route.filters} onApply={(filters) => to({ filters })} />
              <Rows vault={vault} route={route} view={view} />
            </>
          )
        }
      </QueryView>
    </section>
  );
};
