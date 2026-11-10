import { documentPath } from "@memex/core";
import type { Contradiction, Page, VaultId } from "@memex/types";
import { Button, EmptyState } from "@memex/ui";
import { useInfiniteQuery, useQuery } from "@tanstack/react-query";
import { useMemo } from "react";
import { useEngine } from "../engine-context";
import { QUERY_KEYS, unwrap } from "../query";
import { QueryView } from "../shell/query-view";
import { AppLink } from "../wiki/wiki-common";
import { EpistemicOf, TrustOf } from "./review-common";

const ClaimCard = ({ vault, id }: { readonly vault: VaultId; readonly id: string }) => {
  const engine = useEngine();
  const query = useQuery({ queryKey: QUERY_KEYS.lineage(vault, id), queryFn: async () => unwrap(await engine.getLineage(vault, id)) });
  return (
    <div className="flex min-w-0 flex-col gap-2 rounded-md border border-border p-2 text-sm" data-testid="contradiction-claim" data-claim={id}>
      <QueryView query={query} label="this claim">
        {(claim) => (
          <>
            <p className="flex flex-wrap items-center gap-2"><EpistemicOf status={claim.epistemic} /><span className="break-words">{claim.subject} {claim.predicate} <strong>{claim.value}</strong></span></p>
            {claim.evidence.length === 0 && <p className="text-text-muted">It cites nothing that still resolves.</p>}
            <ul className="flex flex-col gap-1">
              {claim.evidence.map((e) => (
                <li key={e.evidenceId} className="flex flex-col gap-0.5">
                  <blockquote className="border-l-2 border-border-strong pl-2 break-words whitespace-pre-wrap">{e.quote}</blockquote>
                  <span className="flex flex-wrap items-center gap-2 text-xs">
                    <TrustOf tier={e.trustTier} />
                    <AppLink to={documentPath(vault, { kind: "source", id: e.sourceId, tab: "view", page: null, zoom: null, claim: id, evidence: e.evidenceId, fromMs: null, toMs: null, rendition: null, segment: null })}>{e.sourceName}</AppLink>
                  </span>
                </li>
              ))}
            </ul>
          </>
        )}
      </QueryView>
    </div>
  );
};

const Pair = ({ vault, pair }: { readonly vault: VaultId; readonly pair: Contradiction }) => (
  <li className="grid grid-cols-2 gap-2" data-testid="contradiction">
    <ClaimCard vault={vault} id={pair.claim} />
    <ClaimCard vault={vault} id={pair.other} />
  </li>
);

export const ContradictionsView = ({ vault }: { readonly vault: VaultId }) => {
  const engine = useEngine();
  const query = useInfiniteQuery({
    queryKey: QUERY_KEYS.contradictions(vault),
    initialPageParam: null as string | null,
    getNextPageParam: (last: Page<Contradiction>) => last.nextCursor,
    queryFn: async ({ pageParam }) => unwrap(await engine.listContradictions(vault, pageParam)),
  });
  const rows = useMemo(() => query.data?.pages.flatMap((p) => p.items) ?? [], [query.data]);
  return (
    <div className="flex flex-col gap-3">
      <p className="text-sm text-text-muted">Both claims stay when they disagree, each with its sources, for a person to weigh.</p>
      <QueryView query={query} label="contradictions" isEmpty={(d) => d.pages.every((p) => p.items.length === 0)} empty={<EmptyState title="No claims contradict each other." />}>
        {() => (
          <>
            <ul className="flex flex-col gap-3">{rows.map((p) => <Pair key={`${p.claim}-${p.other}`} vault={vault} pair={p} />)}</ul>
            {query.hasNextPage && <Button className="self-start" disabled={query.isFetchingNextPage} onClick={() => void query.fetchNextPage()}>Load more</Button>}
          </>
        )}
      </QueryView>
    </div>
  );
};
