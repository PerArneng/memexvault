import { DEFAULT_ZOOM, documentPath, flagText, isNotFound, mediaKind, pageAngle, pageWordsByPage, requestIdText, evidenceState } from "@memex/core";
import type { SourceViewRoute } from "@memex/core";
import type { AnchorRect, DocumentSource, EvidenceStep, PageWord, Rendition, Transcript, VaultId } from "@memex/types";
import { Badge, Button, EmptyState } from "@memex/ui";
import { useInfiniteQuery, useQuery } from "@tanstack/react-query";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { useEngine } from "../engine-context";
import { FailureError, QUERY_KEYS, unwrap } from "../query";
import { QueryView } from "../shell/query-view";
import { MediaPlayer } from "../viewer/media-player";
import type { MediaPlayerHandle } from "../viewer/media-player";
import { PageViewer } from "../viewer/page-viewer";
import { TranscriptView } from "../viewer/transcript-view";
import { AppLink, Callout, failureText, useGo } from "../wiki/wiki-common";
import { failureCode, failureStatus, sourceRoute } from "./document-common";
import { SegmentsTab } from "./source-tabs";

const NO_RECTS: readonly AnchorRect[] = [];
const noWords = (): readonly PageWord[] => [];

/** What a deep link's evidence says about itself. It's what a person needs to know before trusting the highlight. */
export const EvidenceBanner = ({ vault, claim, evidence }: { readonly vault: VaultId; readonly claim: string; readonly evidence: EvidenceStep | null }) => {
  const lineagePath = documentPath(vault, { kind: "lineage", claim, evidence: evidence?.evidenceId ?? null });
  if (evidence === null) {
    return (
      <div data-evidence-state="missing">
        <Callout tone="warning" title="There's nothing to highlight for this link.">
          The claim has no such evidence, or the source it cited has been erased. <AppLink to={lineagePath}>See its lineage</AppLink>
        </Callout>
      </div>
    );
  }
  const state = evidenceState(evidence);
  return (
    <div data-evidence-state={state}>
      {state === "broken" && (
        <Callout tone="danger" title="This anchor is broken: the quote can't be found in the source any more.">
          Nothing is highlighted, as the place it came from would be a guess. It said: <q className="text-text">{evidence.quote}</q> <AppLink to={lineagePath}>See its lineage</AppLink>
        </Callout>
      )}
      {state === "superseded" && (
        <Callout tone="warning" title="This highlight comes from a superseded rendition.">
          A newer reading of this source exists, and the highlight is where the older one found the quote. <AppLink to={lineagePath}>See its lineage</AppLink>
        </Callout>
      )}
      {state === "current" && (
        <Callout tone="info" title="Highlighted quote">
          <q className="text-text">{evidence.quote}</q> <AppLink to={lineagePath}>See its lineage</AppLink>
        </Callout>
      )}
    </div>
  );
};

interface PdfProps {
  readonly vault: VaultId;
  readonly source: DocumentSource;
  readonly renditions: readonly Rendition[];
  readonly route: SourceViewRoute;
  readonly highlights: readonly AnchorRect[];
}

const PdfView = ({ vault, source, renditions, route, highlights }: PdfProps) => {
  const engine = useEngine();
  const go = useGo();
  const [selecting, setSelecting] = useState(false);
  const pagesQuery = useQuery({ queryKey: QUERY_KEYS.pageList(vault, source.id), queryFn: async () => unwrap(await engine.listPages(vault, source.id)) });
  const renditionId = pagesQuery.data?.renditionId;
  // The whole word table comes at once, so it waits to be asked for, and a long document opens without it.
  const wordsQuery = useQuery({
    queryKey: QUERY_KEYS.pageWords(vault, renditionId ?? "none"),
    enabled: selecting && renditionId !== undefined,
    staleTime: Infinity,
    queryFn: async () => pageWordsByPage(unwrap(await engine.listPageWords(vault, renditionId ?? ""))),
  });
  const rendition = renditions.find((r) => r.id === renditionId) ?? null;
  const marks = useMemo(() => new Map((rendition?.pages ?? []).filter((p) => p.flags.length > 0).map((p) => [p.index, p.flags.map(flagText).join("; ")])), [rendition]);
  const words = wordsQuery.data;
  const wordsOnPage = useMemo(() => (selecting && words !== undefined ? (page: number) => words.get(page) ?? noWords() : null), [selecting, words]);

  const pathWith = useCallback((over: Partial<SourceViewRoute>) => documentPath(vault, { ...route, ...over }), [route, vault]);
  const imageUrl = useCallback((page: number, dpi: number) => engine.pageImageUrl(vault, source.id, page, dpi), [engine, source.id, vault]);
  const thumbnailUrl = useCallback((page: number) => engine.thumbnailUrl(vault, source.id, page), [engine, source.id, vault]);
  const explainFailure = useCallback(async (page: number, dpi: number) => {
    const result = await engine.checkPageImage(vault, source.id, page, dpi);
    if (result.ok) return "The server has it now, so this may have been a passing fault.";
    // The server says "try again" for a file it can't read, whether that's passing or not, so the screen says what lasting would mean.
    const lasting = result.failure.kind === "http" && result.failure.status === 503 ? " If this keeps happening, the original file may be missing from the server." : "";
    return `${failureText(result.failure)}${lasting}`;
  }, [engine, source.id, vault]);
  const textAngle = useCallback((page: { readonly index: number; readonly rotation: number }) => pageAngle(rendition, page), [rendition]);

  if (pagesQuery.error instanceof FailureError && isNotFound(pagesQuery.error.failure)) {
    return <EmptyState title="This PDF hasn't been read yet.">It will open here when its rendition is ready.</EmptyState>;
  }
  return (
    <QueryView query={pagesQuery} label="this document's pages" isEmpty={(d) => d.pages.length === 0} empty={<EmptyState title="This PDF has no pages." />}>
      {(list) => (
        <PageViewer
          pages={list.pages}
          allowedDpi={list.allowedDpi}
          imageUrl={imageUrl}
          thumbnailUrl={thumbnailUrl}
          explainFailure={explainFailure}
          highlights={highlights}
          page={route.page ?? (highlights[0]?.page ?? 0)}
          zoom={route.zoom ?? DEFAULT_ZOOM}
          // The first page needs no number, unless a highlight would otherwise take the link back to its own page.
          onPageChange={(page) => go(pathWith({ page: page === 0 && highlights.length === 0 ? null : page }), true)}
          onZoomChange={(zoom) => go(pathWith({ zoom: zoom === DEFAULT_ZOOM ? null : zoom }), true)}
          marks={marks}
          wordsOnPage={wordsOnPage}
          textAngle={textAngle}
          toolbar={
            <>
              <Button aria-pressed={selecting} onClick={() => setSelecting((s) => !s)}>Select text</Button>
              {selecting && wordsQuery.isPending && <span role="status" className="text-xs text-text-muted">Loading the word table</span>}
              {selecting && wordsQuery.isError && <span role="alert" className="text-xs text-danger">Couldn't load the word table.</span>}
            </>
          }
        />
      )}
    </QueryView>
  );
};

const TranscriptPanel = ({ vault, source, timeMs, fromMs, toMs, onSeek }: { readonly vault: VaultId; readonly source: DocumentSource; readonly timeMs: number; readonly fromMs: number | null; readonly toMs: number | null; readonly onSeek: (ms: number) => void }) => {
  const engine = useEngine();
  // Starts a little before a cited time, so what leads up to it is there. Whatever comes after loads as it's reached.
  const startAt = fromMs === null ? null : Math.max(0, fromMs - 60_000);
  const [from, setFrom] = useState<number | null>(startAt);
  // A later link can go to before what's loaded.
  useEffect(() => {
    if (startAt !== null) setFrom((now) => (now !== null && startAt < now ? startAt : now));
  }, [startAt]);
  const query = useInfiniteQuery({
    queryKey: QUERY_KEYS.transcript(vault, source.id, from),
    initialPageParam: null as string | null,
    getNextPageParam: (last: Transcript) => last.nextCursor,
    queryFn: async ({ pageParam }) => unwrap(await engine.getTranscript(vault, source.id, { fromMs: from, cursor: pageParam, words: true })),
  });
  const parts = useMemo(() => query.data?.pages.flatMap((p) => p.parts) ?? [], [query.data]);
  if (failureStatus(query.error) === 409 || failureCode(query.error) === "conflict") {
    return <EmptyState title="This recording hasn't been transcribed yet.">The transcript appears here when it's done, and the recording plays meanwhile.</EmptyState>;
  }
  if (query.error instanceof FailureError && isNotFound(query.error.failure)) {
    return <EmptyState title="This recording hasn't been transcribed yet.">The transcript appears here when it's done, and the recording plays meanwhile.</EmptyState>;
  }
  return (
    <QueryView query={query} label="the transcript">
      {() => (parts.length === 0 && query.data !== undefined && from === null ? (
        <EmptyState title="No speech was found in this recording." />
      ) : (
        <div className="flex flex-col gap-2">
          {from !== null && from > 0 && <Button onClick={() => setFrom(null)}>Show from the start</Button>}
          <TranscriptView parts={parts} timeMs={timeMs} fromMs={fromMs} toMs={toMs} onSeek={onSeek} hasMore={query.hasNextPage} loadingMore={query.isFetchingNextPage} onLoadMore={() => void query.fetchNextPage()} />
        </div>
      ))}
    </QueryView>
  );
};

const RecordingView = ({ vault, source, route, evidence }: { readonly vault: VaultId; readonly source: DocumentSource; readonly route: SourceViewRoute; readonly evidence: EvidenceStep | null }) => {
  const engine = useEngine();
  const player = useRef<MediaPlayerHandle>(null);
  const fromMs = route.fromMs ?? (evidence !== null && !evidence.broken ? evidence.tStartMs : null);
  const toMs = route.toMs ?? (evidence !== null && !evidence.broken ? evidence.tEndMs : null);
  const [timeMs, setTimeMs] = useState(fromMs ?? 0);
  const mediaQuery = useQuery({ queryKey: QUERY_KEYS.media(vault, source.id), queryFn: async () => unwrap(await engine.getMedia(vault, source.id)) });
  const onSeek = useCallback((ms: number) => {
    setTimeMs(ms);
    player.current?.seek(ms);
  }, []);
  const media = mediaQuery.data;
  return (
    <div className="flex flex-col gap-3">
      <MediaPlayer ref={player} src={engine.streamUrl(vault, source.id)} mediaType={source.mediaType} fromMs={fromMs} toMs={toMs} downloadUrl={engine.fileUrl(vault, source.id)} onTime={setTimeMs} />
      {fromMs !== null && <Button className="self-start" onClick={() => player.current?.playRange(fromMs, toMs)}>Play the cited range again</Button>}
      {media !== undefined && media.skippedSpans + media.coarseSpans + media.gaps > 0 && (
        <Callout tone="warning" title="Parts of this transcript are less exact.">
          {media.skippedSpans} stretches have no words, {media.coarseSpans} have rougher times, and {media.gaps} holes in the audio are kept as silence.
        </Callout>
      )}
      <TranscriptPanel vault={vault} source={source} timeMs={timeMs} fromMs={fromMs} toMs={toMs} onSeek={onSeek} />
    </div>
  );
};

/** What a deep link's claim contributes: its evidence on this source. */
const useLinkedEvidence = (vault: VaultId, route: SourceViewRoute) => {
  const engine = useEngine();
  const query = useQuery({
    queryKey: QUERY_KEYS.lineage(vault, route.claim ?? "none"),
    enabled: route.claim !== null,
    queryFn: async () => unwrap(await engine.getLineage(vault, route.claim ?? "")),
  });
  const evidence = query.data?.evidence.find((e) => e.evidenceId === route.evidence) ?? null;
  return { query, evidence };
};

export const SourceViewTab = ({ vault, source, renditions, route }: { readonly vault: VaultId; readonly source: DocumentSource; readonly renditions: readonly Rendition[]; readonly route: SourceViewRoute }) => {
  const { query, evidence } = useLinkedEvidence(vault, route);
  const linked = route.claim !== null;
  const kind = mediaKind(source.mediaType);
  const elsewhere = evidence !== null && evidence.sourceId !== source.id;
  const highlights = evidence !== null && !evidence.broken && !elsewhere ? evidence.rects : NO_RECTS;
  // A link names its page through the claim, so the viewer has to know that before it starts, or it would fetch the wrong page.
  const waiting = linked && query.isPending;
  const failure = query.error instanceof FailureError ? query.error.failure : null;
  return (
    <div className="flex flex-col gap-3">
      {linked && failure !== null && (
        <div data-evidence-state="missing">
          <Callout tone="warning" title="Couldn't follow this link's claim.">
            {isNotFound(failure) ? `It doesn't exist, or it has been erased. ${requestIdText(failure) ?? ""}` : failureText(failure)}
          </Callout>
        </div>
      )}
      {linked && query.data !== undefined && <EvidenceBanner vault={vault} claim={route.claim ?? ""} evidence={evidence} />}
      {elsewhere && (
        <Callout tone="warning" title="This claim's evidence is in another version of this document.">
          <AppLink to={documentPath(vault, sourceRoute(evidence.sourceId, { claim: route.claim, evidence: route.evidence }))}>Open it there</AppLink>
        </Callout>
      )}
      {waiting ? (
        <p role="status" className="text-sm text-text-muted">Finding the highlighted place</p>
      ) : kind === "pdf" ? (
        <PdfView vault={vault} source={source} renditions={renditions} route={route} highlights={highlights} />
      ) : kind === "audio" || kind === "video" ? (
        <RecordingView vault={vault} source={source} route={route} evidence={elsewhere ? null : evidence} />
      ) : kind === "text" ? (
        <SegmentsTab vault={vault} sourceId={source.id} renditions={renditions} renditionId={route.rendition} />
      ) : (
        <EmptyState title="This console has no viewer for this type of file.">
          It's <Badge>{source.mediaType}</Badge>. You can download the original.
        </EmptyState>
      )}
    </div>
  );
};
