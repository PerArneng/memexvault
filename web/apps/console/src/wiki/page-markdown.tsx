import { claimIdsIn, prepareWikiText, sectionPath, wikiPath } from "@memex/core";
import type { WikiToken } from "@memex/core";
import type { VaultId, WikiCitation, WikiClaim } from "@memex/types";
import { Badge, EpistemicBadge, Skeleton, TrustBadge } from "@memex/ui";
import type { EpistemicStatus, TrustTier } from "@memex/ui";
import { SafeMarkdown } from "@memex/ui/markdown";
import { useQueries } from "@tanstack/react-query";
import { Unlink } from "lucide-react";
import { useCallback, useMemo } from "react";
import { useEngine } from "../engine-context";
import { QUERY_KEYS, unwrap } from "../query";
import { AppLink, LINK_TEXT } from "./wiki-common";
import type { LinkResolver } from "./wiki-common";

const EPISTEMIC: readonly string[] = ["verified", "asserted", "inferred", "disputed", "superseded"];
const TIERS: readonly string[] = ["verified", "trusted", "unverified", "untrusted"];

const Epistemic = ({ claim }: { readonly claim: WikiClaim }) => {
  const status = claim.status === "superseded" ? "superseded" : claim.epistemic;
  if (status === null) return null;
  return EPISTEMIC.includes(status) ? <EpistemicBadge status={status as EpistemicStatus} /> : <Badge>{status}</Badge>;
};

const Tier = ({ tier }: { readonly tier: string | null }) => {
  if (tier === null) return <Badge>No source</Badge>;
  return TIERS.includes(tier) ? <TrustBadge tier={tier as TrustTier} /> : <Badge>{tier}</Badge>;
};

/** The least trusted source behind a claim, as that's what a reader should weigh it by. */
const weakestTier = (citations: readonly WikiCitation[]): string | null => {
  const tiers = citations.flatMap((c) => (c.trustTier === null ? [] : [c.trustTier]));
  return [...tiers].sort((a, b) => TIERS.indexOf(b) - TIERS.indexOf(a))[0] ?? null;
};

const claimWords = (claim: WikiClaim): string => [claim.subject, claim.predicate, claim.value].filter((part) => part !== null).join(" ");

const evidenceId = (claimId: string): string => `claim-evidence-${claimId}`;

const ClaimChip = ({ id, claim, insideLink }: { readonly id: string; readonly claim: WikiClaim | undefined; readonly insideLink: boolean }) => {
  if (claim === undefined) {
    return <span className="inline-block h-4 w-24 align-middle"><Skeleton className="h-4 w-24" /><span className="sr-only">Loading a claim</span></span>;
  }
  if (claim.status === "missing") return <Badge tone="danger"><Unlink size={12} aria-hidden={true} /> Missing claim {id}</Badge>;
  const inner = (
    <>
      <span>{claimWords(claim)}</span> <Epistemic claim={claim} /> <Tier tier={weakestTier(claim.citations)} />
    </>
  );
  if (insideLink) return <span>{inner}</span>;
  return (
    <button
      type="button"
      data-claim={id}
      onClick={() => {
        document.getElementById(evidenceId(id))?.focus();
      }}
      className="inline-flex flex-wrap items-center gap-1 rounded-sm border border-border px-1 text-left hover:bg-surface-overlay"
    >
      {inner}
    </button>
  );
};

const WikiLink = ({ vault, token, resolver, insideLink }: { readonly vault: VaultId; readonly token: Extract<WikiToken, { kind: "link" }>; readonly resolver: LinkResolver; readonly insideLink: boolean }) => {
  const label = token.label ?? token.target;
  if (insideLink) return <span>{label}</span>;
  const page = resolver.index.bySlug(token.slug);
  if (page !== null) return <AppLink to={wikiPath(vault, { kind: "read", id: page.id, revision: null })} data-link-state="resolved">{label}</AppLink>;
  if (!resolver.complete) return <AppLink to={wikiPath(vault, { kind: "by_slug", slug: token.slug })} data-link-state="unknown">{label}</AppLink>;
  return (
    <AppLink to={wikiPath(vault, { kind: "by_slug", slug: token.slug })} className="inline-flex items-baseline gap-0.5 text-danger underline decoration-dashed" title={`No page has the slug ${token.slug}`} data-link-state="broken">
      {label}
      <Unlink size={11} aria-hidden={true} className="self-center" />
      <span className="sr-only"> (no such page)</span>
    </AppLink>
  );
};

/** The claims a body names. Those a page read already answered need no call, and a draft's new ones are looked up. */
export const useClaimsFor = (vault: VaultId, source: string, known: readonly WikiClaim[]): ReadonlyMap<string, WikiClaim> => {
  const engine = useEngine();
  const ids = useMemo(() => claimIdsIn(source), [source]);
  const have = useMemo(() => new Map(known.map((c) => [c.claimId, c])), [known]);
  const missing = ids.filter((id) => !have.has(id));
  const answers = useQueries({
    queries: missing.map((id) => ({ queryKey: QUERY_KEYS.wikiClaim(vault, id), queryFn: async () => unwrap(await engine.lookupClaim(vault, id)) })),
  });
  const found = new Map(have);
  answers.forEach((answer, i) => {
    const id = missing[i];
    if (id !== undefined && answer.data !== undefined) found.set(id, answer.data);
  });
  return found;
};

export interface PageMarkdownProps {
  readonly vault: VaultId;
  readonly source: string;
  readonly resolver: LinkResolver;
  readonly claims: ReadonlyMap<string, WikiClaim>;
}

/** One page body as HTML. The reader and the editor's preview both use this and nothing else, so they can't disagree. */
export const PageMarkdown = ({ vault, source, resolver, claims }: PageMarkdownProps) => {
  const prepared = useMemo(() => prepareWikiText(source), [source]);
  const renderInline = useCallback(
    (index: number, insideLink: boolean) => {
      const token = prepared.tokens[index];
      if (token === undefined) return null;
      return token.kind === "link" ? <WikiLink vault={vault} token={token} resolver={resolver} insideLink={insideLink} /> : <ClaimChip id={token.id} claim={claims.get(token.id)} insideLink={insideLink} />;
    },
    [prepared, vault, resolver, claims],
  );
  const rawInline = useCallback((index: number) => prepared.tokens[index]?.raw ?? "", [prepared]);
  return (
    <div data-testid="page-markdown" className="min-w-0 break-words">
      <SafeMarkdown source={prepared.text} renderInline={renderInline} rawInline={rawInline} />
    </div>
  );
};

const CitationRow = ({ vault, citation }: { readonly vault: VaultId; readonly citation: WikiCitation }) => (
  <li className="flex flex-col gap-1 border-t border-border pt-2 first:border-t-0 first:pt-0">
    <q className="text-sm">{citation.quote}</q>
    <span className="flex flex-wrap items-center gap-1.5 text-xs text-text-muted">
      {citation.stance === "refutes" && <Badge tone="warning">Refutes</Badge>}
      <Tier tier={citation.trustTier} />
      {citation.sourceId === null ? (
        <span>{citation.sourceName ?? "Source unavailable"}</span>
      ) : (
        <AppLink to={sectionPath(vault, "documents", citation.sourceId)} className={LINK_TEXT}>{citation.sourceName ?? citation.sourceId}</AppLink>
      )}
      {citation.pageFrom !== null && <span>page {citation.pageFrom + 1}{citation.pageTo !== null && citation.pageTo !== citation.pageFrom ? `–${citation.pageTo + 1}` : ""}</span>}
      {citation.anchorBroken && <Badge tone="warning">Location lost in a newer version</Badge>}
    </span>
  </li>
);

/** Each claim in a body with its evidence. It sits under the text, and a claim's chip moves focus here. */
export const ClaimEvidence = ({ vault, source, claims }: { readonly vault: VaultId; readonly source: string; readonly claims: ReadonlyMap<string, WikiClaim> }) => {
  const ids = useMemo(() => claimIdsIn(source), [source]);
  if (ids.length === 0) return null;
  return (
    <section aria-label="Claims and evidence" data-testid="claim-evidence" className="mt-4 flex flex-col gap-2 border-t border-border pt-3">
      <h3 className="font-medium">Claims and evidence</h3>
      {ids.map((id) => {
        const claim = claims.get(id);
        return (
          <article key={id} id={evidenceId(id)} tabIndex={-1} className="flex flex-col gap-2 rounded-md border border-border p-2">
            {claim === undefined ? (
              <Skeleton className="h-4 w-1/2" />
            ) : claim.status === "missing" ? (
              <p className="text-sm text-danger">There's no claim {id}, or what replaced it has gone too.</p>
            ) : (
              <>
                <p className="flex flex-wrap items-center gap-1.5 text-sm">
                  <span>{claimWords(claim)}</span> <Epistemic claim={claim} />
                </p>
                {claim.status === "superseded" && <p className="text-xs text-text-muted">The page names {id}, which was replaced by {claim.currentClaimId}. This is what stands now.</p>}
                {claim.citations.length === 0 ? <p className="text-xs text-text-muted">No evidence is recorded. A person's claim may have none.</p> : (
                  <ul className="flex flex-col gap-2">
                    {claim.citations.map((c) => <CitationRow key={c.evidenceId} vault={vault} citation={c} />)}
                  </ul>
                )}
              </>
            )}
          </article>
        );
      })}
    </section>
  );
};
