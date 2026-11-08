import { DEFAULT_ZOOM, documentPath, flagText, markQuote, sourcePlace } from "@memex/core";
import type { ReviewEvidence, VaultId } from "@memex/types";
import { EmptyState } from "@memex/ui";
import { useQuery } from "@tanstack/react-query";
import { useCallback, useState } from "react";
import { sourceRoute } from "../documents/document-common";
import { useEngine } from "../engine-context";
import { QUERY_KEYS, unwrap } from "../query";
import { QueryView } from "../shell/query-view";
import { PageViewer } from "../viewer/page-viewer";
import { AppLink, Callout, failureText } from "../wiki/wiki-common";
import { TrustOf } from "./review-common";

const PagesView = ({ vault, source, evidence, page }: { readonly vault: VaultId; readonly source: string; readonly evidence: ReviewEvidence; readonly page: number }) => {
  const engine = useEngine();
  const [current, setCurrent] = useState(page);
  const [zoom, setZoom] = useState(DEFAULT_ZOOM);
  const query = useQuery({ queryKey: QUERY_KEYS.pageList(vault, source), queryFn: async () => unwrap(await engine.listPages(vault, source)) });
  const imageUrl = useCallback((p: number, dpi: number) => engine.pageImageUrl(vault, source, p, dpi), [engine, source, vault]);
  const thumbnailUrl = useCallback((p: number) => engine.thumbnailUrl(vault, source, p), [engine, source, vault]);
  const explainFailure = useCallback(async (p: number, dpi: number) => {
    const result = await engine.checkPageImage(vault, source, p, dpi);
    return result.ok ? "The server has it now, so this may have been a passing fault." : failureText(result.failure);
  }, [engine, source, vault]);
  return (
    <QueryView query={query} label="this document's pages" isEmpty={(d) => d.pages.length === 0} empty={<EmptyState title="This document has no pages." />}>
      {(list) => (
        <PageViewer
          pages={list.pages}
          allowedDpi={list.allowedDpi}
          imageUrl={imageUrl}
          thumbnailUrl={thumbnailUrl}
          explainFailure={explainFailure}
          highlights={evidence.rects}
          page={current}
          zoom={zoom}
          onPageChange={setCurrent}
          onZoomChange={setZoom}
        />
      )}
    </QueryView>
  );
};

/** A quote with no page, in plain text or a recording: the words of its segment, with the quote marked. All of it is plain text. */
const SegmentText = ({ vault, evidence }: { readonly vault: VaultId; readonly evidence: ReviewEvidence }) => {
  const engine = useEngine();
  const query = useQuery({ queryKey: QUERY_KEYS.segment(vault, evidence.segmentId), queryFn: async () => unwrap(await engine.getSegment(vault, evidence.segmentId)) });
  return (
    <QueryView query={query} label="the words this quote comes from">
      {(segment) => {
        const { before, mark, after } = markQuote(segment.text, evidence.quote);
        return (
          <div className="flex flex-col gap-2">
            {segment.speaker !== null && <p className="text-xs text-text-muted">Speaker: {segment.speaker}</p>}
            <p data-testid="segment-text" className="max-h-[32rem] overflow-auto rounded-md border border-border bg-surface p-3 text-sm whitespace-pre-wrap">
              {before}
              {mark !== "" && <mark data-testid="quote-mark" className="rounded-sm bg-warning-surface text-warning">{mark}</mark>}
              {after}
            </p>
            {mark === "" && <Callout tone="warning" title="The quote isn't in these words as written, so nothing is marked.">It said: {evidence.quote}</Callout>}
            {segment.tStartMs !== null && evidence.sourceId !== null && (
              <AppLink to={documentPath(vault, sourceRoute(evidence.sourceId, { fromMs: segment.tStartMs, toMs: segment.tEndMs }))}>Play this part of the recording</AppLink>
            )}
          </div>
        );
      }}
    </QueryView>
  );
};

/** One quote, next to its place in the source. */
export const SourcePane = ({ vault, evidence, at, of }: { readonly vault: VaultId; readonly evidence: ReviewEvidence | null; readonly at: number; readonly of: number }) => {
  if (evidence === null) return <EmptyState title="This item cites no source.">It may be a change to a page or a link. What it does is on the left.</EmptyState>;
  const place = sourcePlace(evidence);
  const openIn = evidence.sourceId === null ? null : documentPath(vault, sourceRoute(evidence.sourceId, place.kind === "pages" ? { page: place.page } : {}));
  return (
    <div className="flex flex-col gap-2" data-testid="source-pane">
      <div className="flex flex-wrap items-center gap-2 text-sm">
        <span className="min-w-0 truncate font-medium" data-testid="source-name">{evidence.sourceName ?? "A source that's no longer here"}</span>
        <TrustOf tier={evidence.trustTier} />
        {of > 1 && <span className="text-xs text-text-muted">Quote {at + 1} of {of}. Press s for the next.</span>}
        {openIn !== null && <span className="ml-auto text-xs"><AppLink to={openIn}>Open in Documents</AppLink></span>}
      </div>
      <blockquote data-testid="evidence-quote" className="border-l-2 border-accent pl-2 text-sm whitespace-pre-wrap">{evidence.quote}</blockquote>
      {evidence.superseded && <Callout tone="warning" title="This document has been processed again.">The quote cites a replaced reading, so this can't be approved as it stands.</Callout>}
      {evidence.pageFlags.length > 0 && <Callout tone="warning" title="The pipeline flagged these pages.">{evidence.pageFlags.map(flagText).join("; ")}</Callout>}
      {place.kind === "pages" ? <PagesView key={`${place.source}-${evidence.segmentId}-${evidence.claimId}`} vault={vault} source={place.source} evidence={evidence} page={place.page} /> : <SegmentText vault={vault} evidence={evidence} />}
    </div>
  );
};
