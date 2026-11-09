import { changeSearch, groupItems, parseSearchState, queryTerms, requestIdText, SEARCH_DEBOUNCE_MS, sectionPath, searchPath, searchProblem, searchRequestOf } from "@memex/core";
import type { SearchRequest, SearchState } from "@memex/core";
import type { EvidencePackage, VaultId } from "@memex/types";
import { Button, EmptyState } from "@memex/ui";
import { keepPreviousData, useQuery } from "@tanstack/react-query";
import { useLocation } from "@tanstack/react-router";
import { useEffect, useMemo, useRef, useState } from "react";
import { useEngine } from "../engine-context";
import { FailureError, QUERY_KEYS, unwrap } from "../query";
import { QueryView } from "../shell/query-view";
import { AppLink, Callout, useGo } from "../wiki/wiki-common";
import { DebugPanel } from "./debug-panel";
import { Filters } from "./filters";
import { ResultItem } from "./result-item";

const hasFilters = (s: SearchState): boolean => s.kinds.length + s.types.length + s.sources.length + s.tiers.length + s.epistemic.length > 0 || s.validAt !== "" || s.asOf !== "";

const NO_FILTERS: Partial<SearchState> = { kinds: [], types: [], sources: [], tiers: [], epistemic: [], validAt: "", asOf: "" };

const PATHS: Readonly<Record<string, string>> = { exact: "an exact lookup", identifier: "an identifier", natural: "a natural-language search", filtered: "filters alone", as_of: "the log, at a past state" };

/** The answer, with what it answered, as results may stay on screen while the next ones load. */
interface Answered {
  readonly q: string;
  readonly result: EvidencePackage;
}

const Results = ({ vault, state, answered, loading }: { readonly vault: VaultId; readonly state: SearchState; readonly answered: Answered; readonly loading: boolean }) => {
  const { result, q } = answered;
  const groups = useMemo(() => groupItems(result.items), [result.items]);
  const terms = useMemo(() => queryTerms(q), [q]);
  const names = useMemo(() => new Map(result.items.flatMap((i) => i.citations.flatMap((c) => (c.sourceId !== null && c.sourceName !== null ? [[c.sourceId, c.sourceName] as const] : [])))), [result.items]);
  const first = searchPath(vault, { ...state, cursor: null });
  const pastTheEnd = result.items.length === 0 && state.cursor !== null;
  return (
    // Keyed by what it answers, so one query's results never pass for another's.
    <div key={q} data-search-results={true} data-search-query={q} {...(loading ? { "data-search-stale": true, "aria-busy": true } : {})} className={`flex flex-col gap-4 ${loading ? "opacity-60" : ""}`}>
      <p role="status" className="text-sm text-text-muted">
        {loading ? "Searching. These are the results for the earlier text." : `${String(result.items.length)} ${result.items.length === 1 ? "result" : "results"}, found through ${PATHS[result.path] ?? result.path}.`}
      </p>
      {result.degraded.length > 0 && <Callout tone="warning" title={`This answer rests on fewer signals: ${result.degraded.join(", ")} couldn't answer.`} />}
      {result.notices.map((notice) => <Callout key={notice} tone="info" title={notice} />)}
      {result.asOf !== null && <Callout tone="info" title={`This is how the vault stood at event ${String(result.asOf)}.`}>Links open what the documents hold now. <AppLink to={searchPath(vault, changeSearch(state, { asOf: "" }))}>Back to the present</AppLink></Callout>}
      {pastTheEnd ? (
        <div data-no-more={true}>
          <EmptyState title="There are no more results."><AppLink to={first}>Go to the first page</AppLink></EmptyState>
        </div>
      ) : result.items.length === 0 ? (
        <div data-nothing-relevant={true}>
          <EmptyState title="Nothing relevant was found.">
            {hasFilters(state) ? <>The filters may be why. <AppLink to={searchPath(vault, changeSearch(state, NO_FILTERS))}>Search without them</AppLink></> : "No document, claim or page in this vault matches this."}
          </EmptyState>
        </div>
      ) : result.lowRelevance ? (
        <div data-nothing-relevant={true}>
          <Callout tone="warning" title="Nothing relevant was found.">These are the closest matches, and none of them looks like an answer.</Callout>
        </div>
      ) : null}
      {groups.map((group) => (
        <section key={group.kind} aria-labelledby={`group-${group.kind}`} className="flex flex-col gap-2">
          <h2 id={`group-${group.kind}`} className="text-sm font-medium text-text-muted">{group.label}</h2>
          {group.items.map((item) => <ResultItem key={`${item.kind}-${item.id}`} vault={vault} item={item} terms={terms} names={names} showScore={state.debug} />)}
        </section>
      ))}
      {result.budget.truncated && <p className="text-xs text-text-muted">This page ended early, at its limit of about {result.budget.limit} tokens, as a text is never cut.</p>}
      {(result.nextCursor !== null || state.cursor !== null) && !pastTheEnd && (
        <nav aria-label="Pages of results" className="flex gap-3 text-sm">
          {state.cursor !== null && <AppLink to={first}>First page</AppLink>}
          {result.nextCursor !== null && <AppLink to={searchPath(vault, { ...state, cursor: result.nextCursor })}>Next page</AppLink>}
        </nav>
      )}
      {state.debug && <DebugPanel result={result} />}
    </div>
  );
};

/** Failures a person can act on, each with the way out. Anything else falls through to the shared error state. */
const Problem = ({ vault, state, error, onRetry }: { readonly vault: VaultId; readonly state: SearchState; readonly error: unknown; readonly onRetry: () => void }) => {
  if (!(error instanceof FailureError)) return null;
  const problem = searchProblem(error.failure, state);
  const id = requestIdText(error.failure);
  switch (problem.kind) {
    case "min_seq_ahead":
      return (
        <div data-search-problem="min_seq_ahead">
          <Callout tone="warning" title={`This link waits for event ${String(problem.minSeq)}, which the vault's log hasn't reached.`}>
            It may be from another vault or a restored copy. <AppLink to={searchPath(vault, changeSearch(state, { minSeq: null }))}>Search without waiting</AppLink> <span className="font-mono text-xs">{id}</span>
          </Callout>
        </div>
      );
    case "not_caught_up":
      return (
        <div data-search-problem="not_caught_up">
          <Callout tone="warning" title="The search indexes haven't caught up with that change yet.">
            <div className="flex flex-wrap items-center gap-2">
              <Button onClick={onRetry}>Try again</Button>
              <AppLink to={searchPath(vault, changeSearch(state, { minSeq: null }))}>Search without waiting</AppLink>
            </div>
          </Callout>
        </div>
      );
    case "bad_cursor":
      return (
        <div data-search-problem="bad_cursor">
          <Callout tone="warning" title="This link's page of results can't be read any more.">
            <AppLink to={searchPath(vault, { ...state, cursor: null })}>Go to the first page</AppLink> <span className="font-mono text-xs">{id}</span>
          </Callout>
        </div>
      );
    case "busy":
      return (
        <div data-search-problem="busy">
          <Callout tone="warning" title="Too many searches of past states are running.">
            <Button onClick={onRetry}>Try again</Button>
          </Callout>
        </div>
      );
    case "other":
      return null;
  }
};

const SearchScreen = ({ vault, rest }: { readonly vault: VaultId; readonly rest: readonly string[] }) => {
  const engine = useEngine();
  const go = useGo();
  const { searchStr } = useLocation();
  const { state, ignored } = useMemo(() => {
    const params = new URLSearchParams(searchStr);
    return parseSearchState((name) => params.get(name));
  }, [searchStr]);

  const [draft, setDraft] = useState(state.q);
  // What the URL last held because of this box, so only somebody else's change, such as going back, replaces what's being typed.
  const written = useRef(state.q);
  useEffect(() => {
    if (state.q !== written.current) {
      written.current = state.q;
      setDraft(state.q);
    }
  }, [state.q]);
  useEffect(() => {
    if (draft === written.current) return undefined;
    const timer = setTimeout(() => {
      written.current = draft;
      go(searchPath(vault, changeSearch(state, { q: draft })), true);
    }, SEARCH_DEBOUNCE_MS);
    return () => clearTimeout(timer);
  }, [draft, go, state, vault]);

  const request: SearchRequest | null = useMemo(() => searchRequestOf(state), [state]);
  const query = useQuery<Answered>({
    queryKey: QUERY_KEYS.search(vault, request),
    enabled: request !== null,
    // The old results stay while new ones load, dimmed and saying so, so the page doesn't jump at every word.
    placeholderData: keepPreviousData,
    meta: { ownsErrors: true },
    // The signal is what cancels this when the text changes: the query loses its screen, and TanStack Query aborts the request.
    queryFn: async ({ signal }) => {
      const asked = request as SearchRequest;
      return { q: asked.q, result: unwrap(await engine.search(vault, asked, signal)) };
    },
  });
  const facets = query.data?.result.facets ?? null;
  const names = useMemo(() => new Map((query.data?.result.items ?? []).flatMap((i) => i.citations.flatMap((c) => (c.sourceId !== null && c.sourceName !== null ? [[c.sourceId, c.sourceName] as const] : [])))), [query.data]);

  if (rest.length > 0) {
    return (
      <EmptyState title="There's nothing at this address.">
        <AppLink to={sectionPath(vault, "search")}>Go to Search</AppLink>
      </EmptyState>
    );
  }
  const handled = query.isError && query.error instanceof FailureError && searchProblem(query.error.failure, state).kind !== "other";
  return (
    <div className="flex flex-col gap-4">
      <form
        role="search"
        onSubmit={(e) => {
          e.preventDefault();
          if (draft !== state.q) {
            written.current = draft;
            go(searchPath(vault, changeSearch(state, { q: draft })));
          } else if (request !== null) void query.refetch();
        }}
        className="flex flex-col gap-1"
      >
        <label htmlFor="search-box" className="text-xs text-text-muted">Search this vault</label>
        <input
          id="search-box"
          type="search"
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          autoFocus={true}
          autoComplete="off"
          spellCheck={false}
          maxLength={2000}
          placeholder="Words, a quoted phrase, or an identifier"
          className="h-10 rounded-md border border-border-strong bg-surface-raised px-3 text-base text-text placeholder:text-text-faint"
        />
      </form>
      {ignored.length > 0 && (
        <div data-search-ignored={ignored.join(",")}>
          <Callout tone="warning" title="Parts of this link couldn't be read, so they're left out.">
            That's {ignored.join(", ")}. <AppLink to={searchPath(vault, state)}>Use the link without them</AppLink>
          </Callout>
        </div>
      )}
      <div className="grid grid-cols-[14rem_minmax(0,1fr)] items-start gap-6">
        <Filters state={state} facets={facets} sourceName={(id) => names.get(id) ?? id} onChange={(next) => go(searchPath(vault, next))} />
        <div className="flex min-w-0 flex-col gap-4">
          {state.minSeq !== null && !handled && <p className="text-xs text-text-muted">Waiting for the indexes to reach event {state.minSeq}. <AppLink to={searchPath(vault, changeSearch(state, { minSeq: null }))}>Don't wait</AppLink></p>}
          {request === null ? (
            <div data-search-empty={true}>
              <EmptyState title="Type something to search for.">Words find passages and claims in any language this vault holds. A quoted phrase must match as written, and an identifier goes straight to its object.</EmptyState>
            </div>
          ) : handled ? (
            <Problem vault={vault} state={state} error={query.error} onRetry={() => void query.refetch()} />
          ) : (
            <QueryView query={query} label="search results">
              {(answered) => <Results vault={vault} state={state} answered={answered} loading={query.isPlaceholderData} />}
            </QueryView>
          )}
        </div>
      </div>
    </div>
  );
};

export default SearchScreen;
