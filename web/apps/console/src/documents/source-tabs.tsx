import { documentPath, flagText, formatClock } from "@memex/core";
import type { CitingClaim, DocumentSource, Page, Rendition, Segment, VaultId } from "@memex/types";
import { Badge, Button, EmptyState } from "@memex/ui";
import { useInfiniteQuery, useQuery } from "@tanstack/react-query";
import { useEngine } from "../engine-context";
import { QUERY_KEYS, unwrap } from "../query";
import { QueryView } from "../shell/query-view";
import { TierOf } from "../viewer/badges";
import { AppLink, Callout, formatTime } from "../wiki/wiki-common";
import { useSourcePath } from "./document-common";

const pageLabel = (from: number, to: number | null): string => (to === null || to === from ? `Page ${String(from + 1)}` : `Pages ${String(from + 1)} to ${String(to + 1)}`);

/** A rendition's segments, whose text is untrusted and stays text. */
export const SegmentsTab = ({ vault, sourceId, renditions, renditionId }: { readonly vault: VaultId; readonly sourceId: string; readonly renditions: readonly Rendition[]; readonly renditionId: string | null }) => {
  const engine = useEngine();
  const pathOf = useSourcePath(vault);
  const current = renditions.find((r) => r.supersededBy === null) ?? renditions[renditions.length - 1];
  const chosen = renditionId === null ? current : renditions.find((r) => r.id === renditionId);
  const query = useInfiniteQuery({
    queryKey: QUERY_KEYS.segments(vault, chosen?.id ?? "none"),
    enabled: chosen !== undefined,
    initialPageParam: null as string | null,
    getNextPageParam: (last: Page<Segment>) => last.nextCursor,
    queryFn: async ({ pageParam }) => unwrap(await engine.listSegments(vault, chosen?.id ?? "", pageParam)),
  });
  if (chosen === undefined) return <EmptyState title={renditionId === null ? "This source hasn't been read yet." : "This source has no such rendition."} />;
  return (
    <section aria-label="Segments" className="flex flex-col gap-2">
      {chosen.supersededBy !== null && (
        <div data-superseded-rendition={true}>
          <Callout tone="warning" title="These segments are from a superseded rendition.">
            A newer reading of this source replaced it. It stays readable, so what cited it can still be checked. <AppLink to={pathOf(sourceId, { tab: "segments" })}>Show the current rendition</AppLink>
          </Callout>
        </div>
      )}
      <QueryView query={query} label="segments" isEmpty={(d) => d.pages.every((p) => p.items.length === 0)} empty={<EmptyState title="This rendition has no text." />}>
        {(data) => (
          <ol className="flex flex-col gap-2">
            {data.pages.flatMap((p) => p.items).map((segment) => (
              <li key={segment.id} data-segment={segment.id} className="rounded-md border border-border p-2">
                <div className="mb-1 flex flex-wrap items-center gap-2 text-xs text-text-muted">
                  {segment.pageFrom !== null && <AppLink to={pathOf(sourceId, { page: segment.pageFrom })}>{pageLabel(segment.pageFrom, segment.pageTo)}</AppLink>}
                  {segment.tStartMs !== null && <AppLink to={pathOf(sourceId, { fromMs: segment.tStartMs, toMs: segment.tEndMs })}>{formatClock(segment.tStartMs)}</AppLink>}
                  {segment.speaker !== null && <Badge>{segment.speaker}</Badge>}
                  {segment.language !== null && <span>{segment.language}</span>}
                  {segment.heading !== null && <span className="font-medium text-text">{segment.heading}</span>}
                </div>
                <p className="text-sm whitespace-pre-wrap">{segment.text}</p>
              </li>
            ))}
          </ol>
        )}
      </QueryView>
      {query.hasNextPage && <Button disabled={query.isFetchingNextPage} onClick={() => void query.fetchNextPage()}>Load more</Button>}
    </section>
  );
};

export const ClaimsTab = ({ vault, sourceId }: { readonly vault: VaultId; readonly sourceId: string }) => {
  const engine = useEngine();
  const pathOf = useSourcePath(vault);
  const query = useInfiniteQuery({
    queryKey: QUERY_KEYS.sourceClaims(vault, sourceId),
    initialPageParam: null as string | null,
    getNextPageParam: (last: Page<CitingClaim>) => last.nextCursor,
    queryFn: async ({ pageParam }) => unwrap(await engine.listSourceClaims(vault, sourceId, pageParam)),
  });
  return (
    <section aria-label="Claims citing this source" className="flex flex-col gap-2">
      <QueryView query={query} label="claims" isEmpty={(d) => d.pages.every((p) => p.items.length === 0)} empty={<EmptyState title="No claims cite this source yet." />}>
        {(data) => (
          <ul className="flex flex-col gap-1">
            {data.pages.flatMap((p) => p.items).map((claim) => (
              <li key={claim.evidenceId} className="flex flex-wrap items-baseline gap-x-3 rounded-md border border-border p-2 text-sm">
                <span className="min-w-0 flex-1 break-words">{claim.subject} · {claim.predicate} · <strong>{claim.value}</strong></span>
                <AppLink to={pathOf(sourceId, { claim: claim.assertionId, evidence: claim.evidenceId })}>Show it in the source</AppLink>
                <AppLink to={documentPath(vault, { kind: "lineage", claim: claim.assertionId, evidence: claim.evidenceId })}>Lineage</AppLink>
              </li>
            ))}
          </ul>
        )}
      </QueryView>
      {query.hasNextPage && <Button disabled={query.isFetchingNextPage} onClick={() => void query.fetchNextPage()}>Load more</Button>}
    </section>
  );
};

export const RenditionsTab = ({ vault, sourceId, renditions }: { readonly vault: VaultId; readonly sourceId: string; readonly renditions: readonly Rendition[] }) => {
  const pathOf = useSourcePath(vault);
  if (renditions.length === 0) return <EmptyState title="This source hasn't been read yet." />;
  return (
    <ul aria-label="Renditions" className="flex flex-col gap-2">
      {[...renditions].reverse().map((r) => {
        const flagged = r.pages.filter((p) => p.flags.length > 0);
        return (
          <li key={r.id} data-rendition={r.id} className="flex flex-col gap-1 rounded-md border border-border p-2 text-sm">
            <div className="flex flex-wrap items-center gap-2">
              {r.supersededBy === null ? <Badge tone="success">Current</Badge> : <Badge tone="warning">Superseded</Badge>}
              <span className="font-mono text-xs break-all">{r.parser}</span>
              <span className="text-xs text-text-muted">pipeline {r.pipelineVersion} · {r.pageCount} pages · {r.segmentCount} segments · {formatTime(r.createdAt)}</span>
              <AppLink to={pathOf(sourceId, { tab: "segments", rendition: r.id })}>Segments</AppLink>
            </div>
            {flagged.length > 0 && (
              <details open={flagged.length <= 20}>
                <summary className="cursor-pointer text-xs text-warning">{flagged.length} flagged pages</summary>
                <ul className="mt-1 flex flex-col gap-0.5 text-xs">
                  {flagged.map((p) => (
                    <li key={p.index}>
                      <AppLink to={pathOf(sourceId, { page: p.index })}>Page {p.index + 1}</AppLink>: {p.flags.map(flagText).join("; ")}
                    </li>
                  ))}
                </ul>
              </details>
            )}
          </li>
        );
      })}
    </ul>
  );
};

export const VersionsTab = ({ vault, source }: { readonly vault: VaultId; readonly source: DocumentSource }) => {
  const engine = useEngine();
  const pathOf = useSourcePath(vault);
  const query = useQuery({ queryKey: QUERY_KEYS.sourceVersions(vault, source.id), queryFn: async () => unwrap(await engine.listSourceVersions(vault, source.id)) });
  return (
    <QueryView query={query} label="versions">
      {(versions) => (
        <ul aria-label="Versions" className="flex flex-col gap-1">
          {[...versions].reverse().map((v) => (
            <li key={v.id} className="flex flex-wrap items-center gap-2 rounded-md border border-border p-2 text-sm">
              <span className="font-medium">Version {v.versionNo}</span>
              {v.id === source.id ? <Badge tone="info">Showing</Badge> : <AppLink to={pathOf(v.id)}>{v.name}</AppLink>}
              <TierOf tier={v.trustTier} />
              <span className="font-mono text-xs text-text-muted break-all">{v.sha256}</span>
              <span className="text-xs text-text-muted">{formatTime(v.recordedAt)}</span>
            </li>
          ))}
        </ul>
      )}
    </QueryView>
  );
};
