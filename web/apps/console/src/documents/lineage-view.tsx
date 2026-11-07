import { documentPath, evidenceState, formatClock, mediaKind } from "@memex/core";
import type { ClaimLineage, EvidenceStep, VaultId } from "@memex/types";
import { Badge, EmptyState } from "@memex/ui";
import { useQuery } from "@tanstack/react-query";
import { Bot, UserRound } from "lucide-react";
import type { ReactNode } from "react";
import { useEngine } from "../engine-context";
import { QUERY_KEYS, unwrap } from "../query";
import { QueryView } from "../shell/query-view";
import { EpistemicOf, TierOf } from "../viewer/badges";
import { AppLink, Callout } from "../wiki/wiki-common";
import { sourceRoute } from "./document-common";

const Step = ({ name, children }: { readonly name: string; readonly children: ReactNode }) => (
  <div className="grid grid-cols-[6.5rem_minmax(0,1fr)] gap-2 border-t border-border py-1.5 text-sm first:border-t-0">
    <dt className="text-xs font-medium text-text-muted">{name}</dt>
    <dd className="min-w-0 break-words">{children}</dd>
  </div>
);

const anchorText = (e: EvidenceStep): string => {
  if (e.tStartMs !== null) return `${formatClock(e.tStartMs)} to ${formatClock(e.tEndMs ?? e.tStartMs)}${e.speaker === null ? "" : `, ${e.speaker}`}`;
  const pages = [...new Set(e.rects.map((r) => r.page + 1))];
  return pages.length === 0 ? "No place in the source" : `Page ${pages.join(", ")}, ${String(e.rects.length)} ${e.rects.length === 1 ? "line" : "lines"}`;
};

/** The model and prompt behind a machine's claim. A person's claim has neither, and the row says so. */
const Derived = ({ vault, lineage, evidence }: { readonly vault: VaultId; readonly lineage: ClaimLineage; readonly evidence: EvidenceStep }) => {
  const engine = useEngine();
  const origin = useQuery({ queryKey: QUERY_KEYS.changeOrigin(vault, lineage.changesetId), queryFn: async () => unwrap(await engine.getChangeOrigin(vault, lineage.changesetId)), staleTime: Infinity });
  const derivation = useQuery({
    queryKey: QUERY_KEYS.derivation(vault, evidence.renditionId, evidence.segmentId),
    enabled: lineage.proposerIsMachine === true,
    queryFn: async () => unwrap(await engine.findDerivation(vault, evidence.renditionId, evidence.segmentId)),
  });
  if (lineage.proposerIsMachine !== true) return <Step name="Derived by">Nothing: a person made this claim.</Step>;
  const found = derivation.data ?? null;
  return (
    <Step name="Derived by">
      <span data-derived={true} className="flex flex-col gap-0.5">
        <span>Model: <span className="font-mono text-xs">{found?.model ?? origin.data?.extractor ?? "not recorded"}</span></span>
        <span>Prompt: <span className="font-mono text-xs break-all">{found?.promptHash ?? origin.data?.promptVersion ?? "not recorded"}</span></span>
        {found !== null && <span className="text-xs text-text-muted">{found.kind}, {found.outcome}</span>}
      </span>
    </Step>
  );
};

const SegmentStep = ({ vault, id }: { readonly vault: VaultId; readonly id: string }) => {
  const engine = useEngine();
  const query = useQuery({ queryKey: QUERY_KEYS.segment(vault, id), queryFn: async () => unwrap(await engine.getSegment(vault, id)) });
  return (
    <Step name="Segment">
      <span className="font-mono text-xs break-all text-text-muted">{id}</span>
      {query.data !== undefined && <p className="mt-1 line-clamp-4 whitespace-pre-wrap">{query.data.text}</p>}
      {query.isError && <p className="text-xs text-danger">This segment can't be read.</p>}
    </Step>
  );
};

const EvidenceCard = ({ vault, lineage, evidence, focused }: { readonly vault: VaultId; readonly lineage: ClaimLineage; readonly evidence: EvidenceStep; readonly focused: boolean }) => {
  const state = evidenceState(evidence);
  const recording = ["audio", "video"].includes(mediaKind(evidence.mediaType));
  const link = documentPath(vault, recording && !evidence.broken && evidence.tStartMs !== null ? sourceRoute(evidence.sourceId, { fromMs: evidence.tStartMs, toMs: evidence.tEndMs }) : sourceRoute(evidence.sourceId, { claim: lineage.claimId, evidence: evidence.evidenceId }));
  return (
    <section aria-label={`Evidence from ${evidence.sourceName}`} data-evidence={evidence.evidenceId} data-evidence-state={state} className={`flex flex-col gap-2 rounded-lg border p-3 ${focused ? "border-accent" : "border-border"}`}>
      {state === "broken" && <Callout tone="danger" title="This anchor is broken: the quote can't be found in the source any more.">Nothing is highlighted, as the place it came from would be a guess.</Callout>}
      {state === "superseded" && <Callout tone="warning" title="This comes from a superseded rendition.">A newer reading of this source exists. This one stays readable.</Callout>}
      <dl>
        <Step name="Evidence">
          <Badge tone={evidence.stance === "supports" ? "success" : "warning"}>{evidence.stance}</Badge> <q data-quote={true} className="whitespace-pre-wrap">{evidence.quote}</q>
        </Step>
        <Step name="Anchor">
          {state === "broken" ? <Badge tone="danger">Broken</Badge> : <span>{anchorText(evidence)} · <AppLink to={link}>{recording ? "Play it" : "Show it in the source"}</AppLink></span>}
        </Step>
        <SegmentStep vault={vault} id={evidence.segmentId} />
        <Step name="Rendition">
          {evidence.superseded ? <Badge tone="warning">Superseded</Badge> : <Badge tone="success">Current</Badge>} <span className="font-mono text-xs break-all">{evidence.parser}</span> <span className="font-mono text-xs break-all text-text-muted">{evidence.renditionId}</span>
        </Step>
        <Step name="Source">
          <AppLink to={documentPath(vault, sourceRoute(evidence.sourceId))}>{evidence.sourceName}</AppLink> <TierOf tier={evidence.trustTier} /> <span className="font-mono text-xs break-all text-text-muted">sha256 {evidence.sourceSha256}</span>
        </Step>
        <Derived vault={vault} lineage={lineage} evidence={evidence} />
      </dl>
    </section>
  );
};

/** From a claim down through Evidence, anchor, Segment and Rendition to the Source. */
export const LineageView = ({ vault, claim, evidence }: { readonly vault: VaultId; readonly claim: string; readonly evidence: string | null }) => {
  const engine = useEngine();
  const query = useQuery({ queryKey: QUERY_KEYS.lineage(vault, claim), queryFn: async () => unwrap(await engine.getLineage(vault, claim)) });
  return (
    <QueryView query={query} label="this claim">
      {(lineage) => (
        <section aria-label="Lineage" className="flex flex-col gap-3">
          <div className="flex flex-col gap-1 rounded-lg border border-border-strong bg-surface-raised p-3">
            <h2 className="font-medium break-words">{lineage.subject} · {lineage.predicate} · {lineage.value}</h2>
            <p className="flex flex-wrap items-center gap-2 text-xs text-text-muted">
              <EpistemicOf status={lineage.epistemic} />
              {lineage.proposerIsMachine === true ? <Badge tone="info"><Bot size={12} aria-hidden={true} /> Agent</Badge> : lineage.proposerIsMachine === false ? <Badge><UserRound size={12} aria-hidden={true} /> Person</Badge> : null}
              {lineage.proposer !== null && <span>proposed by {lineage.proposer}</span>}
              {lineage.decidedBy !== null && <span>decided by {lineage.decidedBy}</span>}
            </p>
            {lineage.rationale !== null && <p className="text-sm text-text-muted">{lineage.rationale}</p>}
            {lineage.supersededBy !== null && (
              <Callout tone="warning" title="Another claim has replaced this one.">
                <AppLink to={documentPath(vault, { kind: "lineage", claim: lineage.supersededBy, evidence: null })}>See what stands now</AppLink>
              </Callout>
            )}
          </div>
          {lineage.evidence.length === 0 ? (
            <div data-evidence-state="missing">
              <EmptyState title="This claim has no evidence that can be followed.">It may have none, or the source it cited may have been erased.</EmptyState>
            </div>
          ) : (
            lineage.evidence.map((e) => <EvidenceCard key={e.evidenceId} vault={vault} lineage={lineage} evidence={e} focused={e.evidenceId === evidence} />)
          )}
        </section>
      )}
    </QueryView>
  );
};
