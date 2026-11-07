import { canWritePages } from "@memex/core";
import type { Identity, VaultId, WikiPage, WikiRevision } from "@memex/types";
import { Button, EmptyState, Input } from "@memex/ui";
import { useQuery } from "@tanstack/react-query";
import { useMemo, useState } from "react";
import type { ChangeEvent } from "react";
import { useEngine } from "../engine-context";
import { QueryView } from "../shell/query-view";
import { QUERY_KEYS, unwrap } from "../query";
import { ClaimEvidence, PageMarkdown, useClaimsFor } from "./page-markdown";
import { AppLink, Callout, ChangeAuthor, downloadText, failureText, formatTime, InlineAction, LINK_BUTTON, useGo, useLinkResolver, useRefreshWiki, wikiPath } from "./wiki-common";

const SHOW_AT_FIRST = 50;

const LinkList = <T,>({ title, items, empty, row }: { readonly title: string; readonly items: readonly T[]; readonly empty: string; readonly row: (item: T) => React.ReactNode }) => {
  const [shown, setShown] = useState(SHOW_AT_FIRST);
  return (
    <section aria-label={title} className="flex flex-col gap-1">
      <h3 className="text-sm font-medium">{title} <span className="font-normal text-text-muted">{items.length}</span></h3>
      {items.length === 0 ? <p className="text-xs text-text-muted">{empty}</p> : (
        <ul className="flex flex-col gap-0.5 text-sm">
          {items.slice(0, shown).map(row)}
        </ul>
      )}
      {items.length > shown && <Button variant="ghost" onClick={() => setShown(shown + SHOW_AT_FIRST)}>Show more</Button>}
    </section>
  );
};

const LinksPanel = ({ vault, id }: { readonly vault: VaultId; readonly id: string }) => {
  const engine = useEngine();
  const backlinks = useQuery({ queryKey: QUERY_KEYS.wikiBacklinks(vault, id), queryFn: async () => unwrap(await engine.listWikiBacklinks(vault, id)) });
  const links = useQuery({ queryKey: QUERY_KEYS.wikiLinks(vault, id), queryFn: async () => unwrap(await engine.listWikiLinks(vault, id)) });
  return (
    <aside aria-label="Links" className="flex w-64 shrink-0 flex-col gap-4">
      <QueryView query={backlinks} label="backlinks">
        {(items) => (
          <LinkList title="Backlinks" items={items} empty="No page links here." row={(b) => (
            <li key={b.edgeId} className="truncate">
              <AppLink to={wikiPath(vault, { kind: "read", id: b.fromPage, revision: null })}>{b.fromTitle ?? b.fromPage}</AppLink>
            </li>
          )} />
        )}
      </QueryView>
      <QueryView query={links} label="outgoing links">
        {(items) => {
          const pages = items.filter((l) => l.kind === "wiki_link" && l.resolved);
          const broken = items.filter((l) => !l.resolved);
          return (
            <>
              <LinkList title="Links from this page" items={pages} empty="This page links to no other page." row={(l) => (
                <li key={l.edgeId} className="truncate">
                  <AppLink to={l.toPage === null ? wikiPath(vault, { kind: "by_slug", slug: l.target }) : wikiPath(vault, { kind: "read", id: l.toPage, revision: null })}>{l.target}</AppLink>
                </li>
              )} />
              <LinkList title="Unresolved links" items={broken} empty="Every link and claim here leads somewhere." row={(l) => (
                <li key={l.edgeId} className="truncate text-text-muted">
                  {l.kind === "wiki_link" ? <AppLink to={wikiPath(vault, { kind: "by_slug", slug: l.target })}>{l.target}</AppLink> : <>Claim {l.target}</>}
                </li>
              )} />
            </>
          );
        }}
      </QueryView>
    </aside>
  );
};

const lastChange = (revisions: readonly WikiRevision[], revision: number): WikiRevision | undefined => revisions.find((r) => r.revision === revision);

type Panel = "rename" | "delete" | "revert" | null;

const Actions = ({ vault, page, reading, writable, onReview }: { readonly vault: VaultId; readonly page: WikiPage; readonly reading: number; readonly writable: boolean; readonly onReview: (changesetId: string | null, what: string, path: string) => void }) => {
  const engine = useEngine();
  const go = useGo();
  const refresh = useRefreshWiki(vault);
  const [panel, setPanel] = useState<Panel>(null);
  const [title, setTitle] = useState(page.title);
  const [slug, setSlug] = useState("");
  const [problem, setProblem] = useState<string | null>(null);
  const here = wikiPath(vault, { kind: "read", id: page.id, revision: null });

  const exportPage = async () => {
    setProblem(null);
    const text = await engine.exportWikiPage(vault, page.id);
    if (text.ok) downloadText(`${page.currentSlug}.md`, text.value);
    else setProblem(failureText(text.failure));
  };
  const importOver = async (e: ChangeEvent<HTMLInputElement>) => {
    const file = e.target.files?.[0];
    e.target.value = "";
    if (file === undefined) return;
    setProblem(null);
    const out = await engine.importWikiPage(vault, { markdown: await file.text(), baseVersion: page.currentRevision, rationale: `Imported ${file.name}` });
    if (out.kind === "failed") setProblem(failureText(out.failure));
    else if (out.kind === "stale") setProblem("The page has changed since this was opened, so the file wasn't imported. It has been reloaded: try again.");
    else if (out.kind === "refused") setProblem(out.reasons.join(" ") || "This file was refused.");
    else if (out.kind === "pending") onReview(out.changesetId, "This import", here);
    await refresh();
  };
  const finished = async (kind: string, changesetId: string | null, what: string, path: string) => {
    setPanel(null);
    await refresh();
    if (kind === "pending") onReview(changesetId, what, path);
    go(path);
  };

  return (
    <div className="flex flex-col gap-2">
      <div className="flex flex-wrap items-center gap-2">
        {writable && <AppLink to={wikiPath(vault, { kind: "edit", id: page.id })} className={LINK_BUTTON}>Edit</AppLink>}
        <AppLink to={wikiPath(vault, { kind: "history", id: page.id })} className={LINK_BUTTON}>History</AppLink>
        <Button onClick={() => void exportPage()}>Export</Button>
        {writable && (
          <>
            <label className={`${LINK_BUTTON} cursor-pointer focus-within:outline-2 focus-within:outline-focus`}>
              Import over this page
              <input type="file" accept=".md,.markdown,text/markdown,text/plain" className="sr-only" onChange={(e) => void importOver(e)} />
            </label>
            <Button onClick={() => setPanel("rename")}>Rename</Button>
            {reading !== page.currentRevision && <Button onClick={() => setPanel("revert")}>Revert to this revision</Button>}
            <Button onClick={() => setPanel("delete")}>Delete</Button>
          </>
        )}
      </div>
      {problem !== null && <Callout tone="danger" title={problem} />}
      {panel === "rename" && (
        <InlineAction
          title="Rename this page"
          description="The old address keeps working, so no link breaks."
          confirmLabel="Rename"
          disabled={title.trim() === ""}
          defaultNote="Renamed in the console"
          run={(rationale) => engine.renameWikiPage(vault, page.id, { baseVersion: page.currentRevision, title: title.trim(), slug: slug.trim() === "" ? null : slug.trim(), rationale })}
          onDone={(c) => {
            if (c.kind === "stale") void refresh();
            else void finished(c.kind, c.kind === "pending" ? c.changesetId : null, "This rename", here);
          }}
          onCancel={() => setPanel(null)}
        >
          <Input label="New title" value={title} onChange={(e) => setTitle(e.target.value)} />
          <Input label="New slug, or empty to make one from the title" value={slug} onChange={(e) => setSlug(e.target.value)} />
        </InlineAction>
      )}
      {panel === "revert" && (
        <InlineAction
          title={`Revert to revision ${String(reading)}`}
          description={`This makes revision ${String(reading)} current again as a new revision. Nothing in the history is lost.`}
          confirmLabel="Revert"
          defaultNote={`Reverted to revision ${String(reading)} in the console`}
          run={(rationale) => engine.revertWikiPage(vault, page.id, { baseVersion: page.currentRevision, revision: reading, rationale })}
          onDone={(c) => {
            if (c.kind === "stale") void refresh();
            else void finished(c.kind, c.kind === "pending" ? c.changesetId : null, "This revert", here);
          }}
          onCancel={() => setPanel(null)}
        />
      )}
      {panel === "delete" && (
        <InlineAction
          title={`Delete ${page.title}?`}
          description="A delete always goes to review first. Once approved, links to this page read as broken until another page takes its slug. Its history stays."
          confirmLabel="Propose delete"
          danger={true}
          defaultNote="Deleted in the console"
          run={(rationale) => engine.deleteWikiPage(vault, page.id, { baseVersion: page.currentRevision, rationale })}
          onDone={(c) => {
            if (c.kind === "stale") void refresh();
            else if (c.kind === "pending") void finished("pending", c.changesetId, "This delete", here);
            else void finished("saved", null, "", wikiPath(vault, { kind: "list", view: "list", filter: "" }));
          }}
          onCancel={() => setPanel(null)}
        />
      )}
    </div>
  );
};

const Body = ({ identity, vault, page, onReview }: { readonly identity: Identity; readonly vault: VaultId; readonly page: WikiPage; readonly onReview: (changesetId: string | null, what: string, path: string) => void }) => {
  const engine = useEngine();
  const resolver = useLinkResolver(vault);
  const claims = useClaimsFor(vault, page.body, page.claims);
  const revisions = useQuery({ queryKey: QUERY_KEYS.wikiRevisions(vault, page.id), queryFn: async () => unwrap(await engine.listWikiRevisions(vault, page.id)) });
  const change = useMemo(() => lastChange(revisions.data ?? [], page.revision), [revisions.data, page.revision]);
  return (
    <div className="flex gap-6">
      <article aria-labelledby="page-title" className="flex min-w-0 flex-1 flex-col gap-3">
        <header className="flex flex-col gap-1">
          <h2 id="page-title" className="text-xl font-medium break-words">{page.title}</h2>
          <p className="flex flex-wrap items-center gap-2 text-xs text-text-muted">
            <span>Revision {page.revision}</span>
            <span>·</span>
            {change === undefined ? <span>{page.creator}</span> : <ChangeAuthor vault={vault} changesetId={change.changesetId} fallback={change.creator} />}
            <span>{formatTime(change?.createdAt ?? page.updatedAt)}</span>
            {/* The page read doesn't say this for the current revision, and the revision's own row does. */}
            {(change?.restoredFrom ?? page.restoredFrom) !== null && <span>Restored from revision {change?.restoredFrom ?? page.restoredFrom}</span>}
            {page.redirects.length > 0 && <span>Also reached as {page.redirects.join(", ")}</span>}
          </p>
        </header>
        <Actions vault={vault} page={page} reading={page.revision} writable={canWritePages(identity, vault)} onReview={onReview} />
        {page.revision !== page.currentRevision && (
          <Callout tone="warning" title={`You're reading revision ${String(page.revision)}. The page is now at revision ${String(page.currentRevision)}.`}>
            <AppLink to={wikiPath(vault, { kind: "read", id: page.id, revision: null })}>Read the current version</AppLink>
          </Callout>
        )}
        {page.body.trim() === "" ? <EmptyState title="This page has no text yet." /> : <PageMarkdown vault={vault} source={page.body} resolver={resolver} claims={claims} />}
        <ClaimEvidence vault={vault} source={page.body} claims={claims} />
      </article>
      <LinksPanel vault={vault} id={page.id} />
    </div>
  );
};

export const PageReader = ({ identity, vault, id, revision, onReview }: { readonly identity: Identity; readonly vault: VaultId; readonly id: string; readonly revision: number | null; readonly onReview: (changesetId: string | null, what: string, path: string) => void }) => {
  const engine = useEngine();
  const query = useQuery({ queryKey: QUERY_KEYS.wikiPage(vault, id, revision), queryFn: async () => unwrap(await engine.getWikiPage(vault, id, revision)) });
  return <QueryView query={query} label="this page">{(page) => <Body key={`${page.id}`} identity={identity} vault={vault} page={page} onReview={onReview} />}</QueryView>;
};
