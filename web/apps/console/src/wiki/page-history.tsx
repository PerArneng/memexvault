import { canWritePages } from "@memex/core";
import type { Identity, VaultId, WikiBlockChange, WikiRevision } from "@memex/types";
import { Badge, Button, EmptyState } from "@memex/ui";
import type { Tone } from "@memex/ui";
import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { useEngine } from "../engine-context";
import { QueryView } from "../shell/query-view";
import { QUERY_KEYS, unwrap } from "../query";
import { AppLink, ChangeAuthor, formatTime, InlineAction, useGo, useRefreshWiki, wikiPath } from "./wiki-common";

const SHOW_AT_FIRST = 50;

const Row = ({ vault, id, row, current, writable, onReview }: { readonly vault: VaultId; readonly id: string; readonly row: WikiRevision; readonly current: number; readonly writable: boolean; readonly onReview: (changesetId: string | null, what: string, path: string) => void }) => {
  const engine = useEngine();
  const go = useGo();
  const refresh = useRefreshWiki(vault);
  const [reverting, setReverting] = useState(false);
  const here = wikiPath(vault, { kind: "read", id, revision: null });
  return (
    <li className="flex flex-col gap-2 border-b border-border py-2">
      <div className="flex flex-wrap items-center gap-3 text-sm">
        <AppLink to={wikiPath(vault, { kind: "read", id, revision: row.revision })}>Revision {row.revision}</AppLink>
        <span className="min-w-0 truncate">{row.title}</span>
        <ChangeAuthor vault={vault} changesetId={row.changesetId} fallback={row.creator} />
        <time className="font-mono text-xs text-text-muted tabular-nums">{formatTime(row.createdAt)}</time>
        {row.restoredFrom !== null && <Badge>Restored from {row.restoredFrom}</Badge>}
        {row.revision === current && <Badge tone="success">Current</Badge>}
        <span className="ml-auto flex items-center gap-3 text-xs">
          {row.revision > 1 && <AppLink to={wikiPath(vault, { kind: "diff", id, from: row.revision - 1, to: row.revision })}>Changes</AppLink>}
          {row.revision !== current && <AppLink to={wikiPath(vault, { kind: "diff", id, from: row.revision, to: current })}>Compare with current</AppLink>}
          {writable && row.revision !== current && <Button aria-label={`Revert to revision ${String(row.revision)}`} onClick={() => setReverting(true)}>Revert</Button>}
        </span>
      </div>
      {reverting && (
        <InlineAction
          title={`Revert to revision ${String(row.revision)}`}
          description={`This makes revision ${String(row.revision)} current again as a new revision. Nothing in the history is lost.`}
          confirmLabel="Revert"
          defaultNote={`Reverted to revision ${String(row.revision)} in the console`}
          run={(rationale) => engine.revertWikiPage(vault, id, { baseVersion: current, revision: row.revision, rationale })}
          onDone={(c) => {
            void refresh().then(() => {
              if (c.kind === "stale") return;
              setReverting(false);
              if (c.kind === "pending") onReview(c.changesetId, "This revert", here);
              go(here);
            });
          }}
          onCancel={() => setReverting(false)}
        />
      )}
    </li>
  );
};

export const PageHistory = ({ identity, vault, id, onReview }: { readonly identity: Identity; readonly vault: VaultId; readonly id: string; readonly onReview: (changesetId: string | null, what: string, path: string) => void }) => {
  const engine = useEngine();
  const [shown, setShown] = useState(SHOW_AT_FIRST);
  const query = useQuery({ queryKey: QUERY_KEYS.wikiRevisions(vault, id), queryFn: async () => unwrap(await engine.listWikiRevisions(vault, id)) });
  return (
    <section aria-label="History" className="flex flex-col gap-2">
      <div className="flex items-center justify-between">
        <h2 className="text-lg font-medium">History</h2>
        <AppLink to={wikiPath(vault, { kind: "read", id, revision: null })}>Back to the page</AppLink>
      </div>
      <QueryView query={query} label="this page's history" isEmpty={(rows) => rows.length === 0} empty={<EmptyState title="This page has no recorded revisions." />}>
        {(rows) => {
          const newestFirst = [...rows].reverse();
          const current = newestFirst[0]?.revision ?? 0;
          return (
            <>
              <ol className="flex flex-col">
                {newestFirst.slice(0, shown).map((row) => <Row key={row.revision} vault={vault} id={id} row={row} current={current} writable={canWritePages(identity, vault)} onReview={onReview} />)}
              </ol>
              {newestFirst.length > shown && <Button variant="ghost" onClick={() => setShown(shown + SHOW_AT_FIRST)}>Show older revisions</Button>}
            </>
          );
        }}
      </QueryView>
    </section>
  );
};

const CHANGE_TONE: Readonly<Record<string, Tone>> = { added: "success", removed: "danger", changed: "warning", moved: "info" };

/** Raw text, never rendered, as a diff is about the source. */
const Side = ({ label, text }: { readonly label: string; readonly text: string | null }) =>
  text === null ? null : (
    <div className="flex min-w-0 flex-1 flex-col gap-0.5">
      <span className="text-xs text-text-muted">{label}</span>
      <pre tabIndex={0} className="max-h-64 overflow-auto rounded-md bg-surface-raised p-2 font-mono text-xs break-words whitespace-pre-wrap">{text}</pre>
    </div>
  );

const ChangeRow = ({ change }: { readonly change: WikiBlockChange }) => (
  <li className="flex flex-col gap-1 border-b border-border py-2">
    <Badge tone={CHANGE_TONE[change.change] ?? "neutral"}>{change.change}</Badge>
    <div className="flex gap-3">
      <Side label="Before" text={change.before} />
      <Side label="After" text={change.after} />
    </div>
  </li>
);

export const PageDiff = ({ vault, id, from, to }: { readonly vault: VaultId; readonly id: string; readonly from: number; readonly to: number }) => {
  const engine = useEngine();
  const query = useQuery({ queryKey: QUERY_KEYS.wikiDiff(vault, id, from, to), queryFn: async () => unwrap(await engine.getWikiDiff(vault, id, from, to)) });
  return (
    <section aria-label="Changes" className="flex flex-col gap-2">
      <div className="flex items-center justify-between">
        <h2 className="text-lg font-medium">Revision {from} to revision {to}</h2>
        <AppLink to={wikiPath(vault, { kind: "history", id })}>Back to history</AppLink>
      </div>
      <QueryView query={query} label="these changes">
        {(diff) => (
          <>
            {diff.title !== null && <p className="text-sm">Title: {diff.title[0]} → {diff.title[1]}</p>}
            {diff.slug !== null && <p className="text-sm">Slug: {diff.slug[0]} → {diff.slug[1]}</p>}
            {diff.blocks.length === 0 && diff.title === null && diff.slug === null ? <EmptyState title="These revisions are the same." /> : (
              <ul className="flex flex-col">
                {diff.blocks.map((b) => <ChangeRow key={`${b.blockId}-${b.change}`} change={b} />)}
              </ul>
            )}
          </>
        )}
      </QueryView>
    </section>
  );
};
