import { canWritePages, filterPages, pageTree } from "@memex/core";
import type { Identity, VaultId, WikiPageSummary } from "@memex/types";
import { Button, DataTable, EmptyState, Input } from "@memex/ui";
import type { Column } from "@memex/ui";
import { useQuery } from "@tanstack/react-query";
import { useMemo, useState } from "react";
import type { ChangeEvent } from "react";
import { useEngine } from "../engine-context";
import { QueryView } from "../shell/query-view";
import { QUERY_KEYS, unwrap } from "../query";
import { AppLink, Callout, failureText, formatTime, LINK_BUTTON, useGo, useRefreshWiki, useWikiPages, wikiPath } from "./wiki-common";

interface ListRow {
  readonly page: WikiPageSummary;
  readonly depth: number;
}

const INDENT_REM = 1;

const columnsFor = (vault: VaultId): readonly Column<ListRow>[] => [
  {
    id: "title",
    header: "Title",
    width: "minmax(0, 2fr)",
    cell: ({ page, depth }) => (
      <span className="block truncate" style={{ paddingLeft: `${String(depth * INDENT_REM)}rem` }}>
        <AppLink to={wikiPath(vault, { kind: "read", id: page.id, revision: null })}>{page.title}</AppLink>
      </span>
    ),
  },
  { id: "slug", header: "Slug", width: "minmax(0, 1.5fr)", cell: ({ page }) => <span className="font-mono text-xs text-text-muted">{page.slug}</span> },
  { id: "creator", header: "Created by", width: "minmax(0, 1fr)", cell: ({ page }) => <span className="text-xs text-text-muted">{page.creator}</span> },
  { id: "updated", header: "Updated", width: "11rem", cell: ({ page }) => <span className="font-mono text-xs text-text-muted tabular-nums">{formatTime(page.updatedAt)}</span> },
  { id: "version", header: "Revision", width: "5rem", numeric: true, cell: ({ page }) => page.version },
];

export const PageList = ({ identity, vault, view, filter, onReview }: { readonly identity: Identity; readonly vault: VaultId; readonly view: "list" | "tree"; readonly filter: string; readonly onReview: (changesetId: string | null, what: string, path: string) => void }) => {
  const engine = useEngine();
  const go = useGo();
  const refresh = useRefreshWiki(vault);
  const query = useWikiPages(vault);
  const [problem, setProblem] = useState<string | null>(null);
  const writable = canWritePages(identity, vault);
  const columns = useMemo(() => columnsFor(vault), [vault]);

  const importFile = async (e: ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];
    e.target.value = "";
    if (file === undefined) return;
    setProblem(null);
    const out = await engine.importWikiPage(vault, { markdown: await file.text(), baseVersion: null, rationale: `Imported ${file.name}` });
    if (out.kind === "failed") setProblem(failureText(out.failure));
    else if (out.kind === "refused") setProblem(out.reasons.join(" ") || "This file was refused.");
    else if (out.kind === "stale") setProblem("A page with this slug has changed. Open it and import over it.");
    else {
      await refresh();
      if (out.pageId !== undefined) {
        const path = wikiPath(vault, { kind: "read", id: out.pageId, revision: null });
        if (out.kind === "pending") onReview(out.changesetId, "This import", path);
        go(path);
      }
    }
  };

  return (
    <section aria-label="All pages" className="flex flex-col gap-3">
      <div className="flex flex-wrap items-end gap-2">
        <div className="w-64">
          <Input label="Filter by title or slug" value={filter} onChange={(e) => go(wikiPath(vault, { kind: "list", view, filter: e.target.value }), true)} />
        </div>
        <div role="group" aria-label="Show as" className="flex">
          <Button aria-pressed={view === "list"} className="rounded-r-none" onClick={() => go(wikiPath(vault, { kind: "list", view: "list", filter }), true)}>List</Button>
          <Button aria-pressed={view === "tree"} className="rounded-l-none" onClick={() => go(wikiPath(vault, { kind: "list", view: "tree", filter }), true)}>Tree</Button>
        </div>
        <AppLink to={wikiPath(vault, { kind: "broken_links" })} className={LINK_BUTTON}>Broken links</AppLink>
        {writable && (
          <span className="ml-auto flex items-center gap-2">
            <label className={`${LINK_BUTTON} cursor-pointer focus-within:outline-2 focus-within:outline-focus`}>
              Import
              <input type="file" accept=".md,.markdown,text/markdown,text/plain" className="sr-only" onChange={(e) => void importFile(e)} />
            </label>
            <AppLink to={wikiPath(vault, { kind: "new", title: "" })} className={LINK_BUTTON}>New page</AppLink>
          </span>
        )}
      </div>
      {problem !== null && <Callout tone="danger" title={problem} />}
      <QueryView
        query={query}
        label="pages"
        isEmpty={(list) => list.pages.length === 0}
        empty={<EmptyState title="No pages yet.">{writable ? <AppLink to={wikiPath(vault, { kind: "new", title: "" })}>Write the first one</AppLink> : "Someone with write access can add the first one."}</EmptyState>}
      >
        {(list) => {
          const matching = filterPages(list.pages, filter);
          // A tree of a filtered list would be full of false roots, so a filter always shows a plain list.
          const rows: readonly ListRow[] = view === "tree" && filter.trim() === "" ? pageTree(matching) : [...matching].sort((a, b) => a.title.localeCompare(b.title, "en")).map((page) => ({ page, depth: 0 }));
          return (
            <>
              {list.truncated && <Callout tone="warning" title={`Showing the first ${String(list.pages.length)} pages. Filters and link suggestions can't see the rest.`} />}
              {rows.length === 0 ? <EmptyState title="No page matches this filter." /> : <DataTable label="Pages" rows={rows} columns={columns} rowKey={(r) => r.page.id} height={560} />}
            </>
          );
        }}
      </QueryView>
    </section>
  );
};

export const BrokenLinks = ({ vault }: { readonly vault: VaultId }) => {
  const engine = useEngine();
  const query = useQuery({ queryKey: QUERY_KEYS.wikiUnresolved(vault), queryFn: async () => unwrap(await engine.listWikiUnresolved(vault)) });
  const pages = useWikiPages(vault);
  const titles = useMemo(() => new Map((pages.data?.pages ?? []).map((p) => [p.id, p.title])), [pages.data]);
  return (
    <section aria-label="Broken links" className="flex flex-col gap-2">
      <div className="flex items-center justify-between">
        <h2 className="text-lg font-medium">Broken links</h2>
        <AppLink to={wikiPath(vault, { kind: "list", view: "list", filter: "" })}>Back to pages</AppLink>
      </div>
      <QueryView query={query} label="broken links" isEmpty={(items) => items.length === 0} empty={<EmptyState title="Every link and claim reference leads somewhere." />}>
        {(items) => (
          <ul className="flex flex-col gap-1 text-sm">
            {items.map((u) => (
              <li key={u.edgeId} className="flex flex-wrap gap-2">
                {u.kind === "wiki_link" ? <AppLink to={wikiPath(vault, { kind: "by_slug", slug: u.target })}>{u.target}</AppLink> : <span>Claim <span className="font-mono text-xs">{u.target}</span></span>}
                <span className="text-text-muted">in</span>
                <AppLink to={wikiPath(vault, { kind: "read", id: u.fromPage, revision: null })}>{titles.get(u.fromPage) ?? u.fromPage}</AppLink>
              </li>
            ))}
          </ul>
        )}
      </QueryView>
    </section>
  );
};
