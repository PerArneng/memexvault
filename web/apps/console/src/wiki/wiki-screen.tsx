import { canWritePages, isNotFound } from "@memex/core";
import type { Identity, VaultId } from "@memex/types";
import { EmptyState } from "@memex/ui";
import { useQuery } from "@tanstack/react-query";
import { useLocation } from "@tanstack/react-router";
import { useCallback, useEffect, useState } from "react";
import { useEngine } from "../engine-context";
import { FailureError, QUERY_KEYS, unwrap } from "../query";
import { PermissionMessage, QueryView } from "../shell/query-view";
import { EditPage, NewPage } from "./page-editor";
import { PageDiff, PageHistory } from "./page-history";
import { BrokenLinks, PageList } from "./page-list";
import { PageReader } from "./page-reader";
import { AppLink, ReviewNotice, routeFrom, useGo, wikiPath } from "./wiki-common";

const BySlug = ({ vault, slug, writable }: { readonly vault: VaultId; readonly slug: string; readonly writable: boolean }) => {
  const engine = useEngine();
  const go = useGo();
  const query = useQuery({ queryKey: QUERY_KEYS.wikiSlug(vault, slug), queryFn: async () => unwrap(await engine.resolveWikiSlug(vault, slug)) });
  const pageId = query.data?.pageId;
  useEffect(() => {
    if (pageId !== undefined) go(wikiPath(vault, { kind: "read", id: pageId, revision: null }), true);
  }, [go, pageId, vault]);
  if (query.error instanceof FailureError && isNotFound(query.error.failure)) {
    return (
      <EmptyState title={`No page has the slug ${slug} yet.`}>
        {writable ? <AppLink to={wikiPath(vault, { kind: "new", title: slug })}>Create it</AppLink> : "Someone with write access can create it."}
      </EmptyState>
    );
  }
  return <QueryView query={query} label="this page">{() => null}</QueryView>;
};

interface Held {
  readonly path: string;
  readonly changesetId: string | null;
  readonly what: string;
}

const WikiScreen = ({ identity, vault, rest }: { readonly identity: Identity; readonly vault: VaultId; readonly rest: readonly string[] }) => {
  const location = useLocation();
  const route = routeFrom(rest, location.searchStr);
  const writable = canWritePages(identity, vault);
  // A write held for review has nothing on the page to show for it, so the screen it lands on says so.
  const [held, setHeld] = useState<Held | null>(null);
  const onReview = useCallback((changesetId: string | null, what: string, path: string) => setHeld({ changesetId, what, path }), []);
  const notice = held !== null && held.path === `${location.pathname}${location.searchStr}` ? <ReviewNotice vault={vault} changesetId={held.changesetId} what={held.what} /> : null;

  const needsWrite = (route.kind === "new" || route.kind === "edit") && !writable;
  return (
    <div className="flex flex-col gap-3">
      {notice}
      {needsWrite && <PermissionMessage title="You don't have permission to edit pages in this vault.">It needs <strong>propose</strong> scope, and your token has <strong>read</strong>.</PermissionMessage>}
      {route.kind === "list" && <PageList identity={identity} vault={vault} view={route.view} filter={route.filter} onReview={onReview} />}
      {route.kind === "broken_links" && <BrokenLinks vault={vault} />}
      {route.kind === "new" && writable && <NewPage key={route.title} vault={vault} initialTitle={route.title} onReview={onReview} />}
      {route.kind === "by_slug" && <BySlug vault={vault} slug={route.slug} writable={writable} />}
      {route.kind === "read" && <PageReader key={`${route.id}-${String(route.revision)}`} identity={identity} vault={vault} id={route.id} revision={route.revision} onReview={onReview} />}
      {route.kind === "edit" && writable && <EditPage key={route.id} vault={vault} id={route.id} onReview={onReview} />}
      {route.kind === "history" && <PageHistory identity={identity} vault={vault} id={route.id} onReview={onReview} />}
      {route.kind === "diff" && <PageDiff vault={vault} id={route.id} from={route.from} to={route.to} />}
      {route.kind === "unknown" && (
        <EmptyState title="There's nothing at this address.">
          <AppLink to={wikiPath(vault, { kind: "list", view: "list", filter: "" })}>Go to Pages</AppLink>
        </EmptyState>
      )}
    </div>
  );
};

export default WikiScreen;
