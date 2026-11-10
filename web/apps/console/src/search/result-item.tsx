import { citationPath, documentPath, formatClock, itemPath, markTerms } from "@memex/core";
import type { EvidenceItem, SearchCitation, SearchConflict, VaultId } from "@memex/types";
import { Badge, ConflictBadge } from "@memex/ui";
import { useQuery } from "@tanstack/react-query";
import { memo, useMemo } from "react";
import { sourceRoute } from "../documents/document-common";
import { useEngine } from "../engine-context";
import { QUERY_KEYS, unwrap } from "../query";
import { EpistemicOf, TierOf } from "../viewer/badges";
import { MarkedText } from "../viewer/marked-text";
import { AppLink, formatTime } from "../wiki/wiki-common";

const placeOf = (c: SearchCitation): string | null => {
  if (c.tStartMs !== null) return `at ${formatClock(c.tStartMs)}${c.tEndMs === null ? "" : ` to ${formatClock(c.tEndMs)}`}`;
  if (c.pageFrom === null) return null;
  return c.pageTo === null || c.pageTo === c.pageFrom ? `page ${String(c.pageFrom + 1)}` : `pages ${String(c.pageFrom + 1)} to ${String(c.pageTo + 1)}`;
};

const Citation = ({ vault, item, citation }: { readonly vault: VaultId; readonly item: EvidenceItem; readonly citation: SearchCitation }) => {
  const to = citationPath(vault, item, citation);
  const name = citation.sourceName ?? "An unnamed source";
  const place = placeOf(citation);
  return (
    <li data-citation={citation.segmentId ?? citation.evidenceId ?? "none"} data-anchor={citation.anchor} className="flex flex-col gap-1 border-l-2 border-border-strong pl-2 text-sm">
      <div className="flex flex-wrap items-center gap-2 text-xs text-text-muted">
        {to === null ? <span>{citation.sourceName ?? "The source of this is gone."}</span> : <AppLink to={to} data-link-state="citation" title={`Open ${name}${place === null ? "" : `, ${place}`}`}>{name}{place === null ? "" : `, ${place}`}</AppLink>}
        {citation.trustTier !== null && <TierOf tier={citation.trustTier} />}
        {citation.speaker !== null && <Badge>{citation.speaker}</Badge>}
        {citation.anchor === "broken" && <Badge tone="danger">Anchor broken</Badge>}
        {citation.anchor === "none" && citation.sourceId !== null && <Badge>No exact place</Badge>}
      </div>
      {citation.quote !== null && (
        <blockquote data-quote={true} className="text-text whitespace-pre-wrap"><MarkedText parts={[{ text: citation.quote, marked: true }]} /></blockquote>
      )}
      {citation.anchor === "broken" && <p className="text-xs text-danger">The quote can't be found in the source any more, so the link shows the claim's history instead of a place.</p>}
    </li>
  );
};

/** A source's name, from this answer if it's there, and otherwise from the API, since a conflict gives only ids. */
const ConflictSource = ({ vault, id, names }: { readonly vault: VaultId; readonly id: string; readonly names: ReadonlyMap<string, string> }) => {
  const engine = useEngine();
  const known = names.get(id);
  const query = useQuery({ queryKey: QUERY_KEYS.source(vault, id), enabled: known === undefined, queryFn: async () => unwrap(await engine.getSource(vault, id)) });
  return <AppLink to={documentPath(vault, sourceRoute(id))}>{known ?? query.data?.name ?? id}</AppLink>;
};

const ConflictRow = ({ vault, conflict, names }: { readonly vault: VaultId; readonly conflict: SearchConflict; readonly names: ReadonlyMap<string, string> }) => (
  <li data-conflict={conflict.claimId} className="flex flex-col gap-1">
    <div className="flex flex-wrap items-center gap-2">
      <span data-conflict-text={true} className="text-sm text-text whitespace-pre-wrap">{conflict.text}</span>
      <EpistemicOf status={conflict.epistemic} />
    </div>
    <div className="flex flex-wrap items-center gap-2 text-xs text-text-muted">
      <span>From</span>
      {conflict.sourceIds.length === 0 && <span>no source that's still available</span>}
      {conflict.sourceIds.map((id) => <ConflictSource key={id} vault={vault} id={id} names={names} />)}
      <AppLink to={documentPath(vault, { kind: "lineage", claim: conflict.claimId, evidence: null })}>See its evidence</AppLink>
    </div>
  </li>
);

export interface ResultItemProps {
  readonly vault: VaultId;
  readonly item: EvidenceItem;
  readonly terms: readonly string[];
  /** Source names this answer already holds. */
  readonly names: ReadonlyMap<string, string>;
  readonly showScore: boolean;
}

const ITEM_LINK: Readonly<Record<string, string>> = { claim: "See its history", page: "Open the page", block: "Open the page", entity: "Open the entity" };

/** One item of the evidence package. Everything in it is vault content, so it's only ever text. */
export const ResultItem = memo(function ResultItem({ vault, item, terms, names, showScore }: ResultItemProps) {
  const parts = useMemo(() => markTerms(item.text, terms), [item.text, terms]);
  const to = itemPath(vault, item);
  return (
    <article data-item-kind={item.kind} data-item-id={item.id} className="flex flex-col gap-2 rounded-lg border border-border bg-surface-raised p-3 [content-visibility:auto] [contain-intrinsic-size:auto_16rem]">
      <header className="flex flex-wrap items-center gap-2">
        {item.heading !== null && <h3 className="text-sm font-medium break-words">{item.heading}</h3>}
        {[...new Set(item.trustTiers)].map((tier) => <TierOf key={tier} tier={tier} />)}
        <EpistemicOf status={item.epistemic} />
        {item.conflicts.length > 0 && <ConflictBadge count={item.conflicts.length} />}
        {item.language !== null && <span className="text-xs text-text-muted">{item.language}</span>}
        {(item.validFromMs !== null || item.validToMs !== null) && (
          <span className="text-xs text-text-muted">Valid {item.validFromMs === null ? "until" : `from ${formatTime(item.validFromMs)}`}{item.validFromMs !== null && item.validToMs !== null ? " until" : ""}{item.validToMs === null ? "" : ` ${formatTime(item.validToMs)}`}</span>
        )}
        {showScore && item.score !== null && <span className="font-mono text-xs text-text-muted">score {item.score.toFixed(4)}</span>}
        {to !== null && <span className="ml-auto text-xs"><AppLink to={to}>{ITEM_LINK[item.kind] ?? "Open"}</AppLink></span>}
      </header>
      <p data-item-text={true} {...(item.language === null ? {} : { lang: item.language })} className="text-sm whitespace-pre-wrap break-words"><MarkedText parts={parts} /></p>
      {item.citations.length > 0 && (
        <ul aria-label="Where this comes from" className="flex flex-col gap-2">
          {item.citations.map((citation, i) => <Citation key={`${citation.evidenceId ?? citation.segmentId ?? "none"}-${String(i)}`} vault={vault} item={item} citation={citation} />)}
        </ul>
      )}
      {item.conflicts.length > 0 && (
        <section aria-label="What disagrees with this" data-conflicts={true} className="flex flex-col gap-2 rounded-md border border-warning bg-warning-surface p-2">
          <h4 className="text-xs font-medium text-warning">Disagrees with</h4>
          <ul className="flex flex-col gap-2">
            {item.conflicts.map((conflict) => <ConflictRow key={conflict.claimId} vault={vault} conflict={conflict} names={names} />)}
          </ul>
        </section>
      )}
    </article>
  );
});

