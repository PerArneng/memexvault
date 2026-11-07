import { draftStanding, rankPages } from "@memex/core";
import type { Draft, SaveOutcome } from "@memex/core";
import type { VaultId, WikiClaim, WikiPage } from "@memex/types";
import { Button, Input, Skeleton, SplitPane } from "@memex/ui";
import { useQuery } from "@tanstack/react-query";
import { lazy, Suspense, useCallback, useDeferredValue, useEffect, useMemo, useRef, useState } from "react";
import { useEngine } from "../engine-context";
import { QueryView } from "../shell/query-view";
import { QUERY_KEYS, unwrap } from "../query";
import { ConflictView } from "./conflict-view";
import type { EditorOption } from "./markdown-editor";
import { ClaimEvidence, PageMarkdown, useClaimsFor } from "./page-markdown";
import { AppLink, Callout, DEFAULT_NOTE, failureText, formatTime, useGo, useLinkResolver, useRefreshWiki, useWikiPages, wikiPath } from "./wiki-common";

// CodeMirror is most of this screen's weight, and the reader doesn't need it.
const MarkdownEditor = lazy(() => import("./markdown-editor"));

const DRAFT_TICK_MS = 250;

type Conflict = Extract<SaveOutcome, { kind: "conflict" }>;

const useDraftKeeper = (vault: VaultId, page: string) => {
  const engine = useEngine();
  const keeper = useMemo(() => engine.draftKeeper(vault, page), [engine, vault, page]);
  useEffect(() => {
    const flush = () => keeper.flush();
    const onVisibility = () => {
      if (document.visibilityState === "hidden") keeper.flush();
    };
    const timer = setInterval(() => keeper.flushIfDue(), DRAFT_TICK_MS);
    window.addEventListener("pagehide", flush);
    document.addEventListener("visibilitychange", onVisibility);
    return () => {
      clearInterval(timer);
      window.removeEventListener("pagehide", flush);
      document.removeEventListener("visibilitychange", onVisibility);
      keeper.flush();
    };
  }, [keeper]);
  return keeper;
};

const useSuggest = (vault: VaultId, page: WikiPage | null) => {
  const engine = useEngine();
  const { data } = useWikiPages(vault);
  const pages = data?.pages;
  const onPage = page?.claims;
  return useCallback(
    async (kind: "link" | "claim", query: string): Promise<readonly EditorOption[]> => {
      if (kind === "link") {
        return rankPages(pages ?? [], query).map(({ page: p, viaRedirect }) => ({ value: viaRedirect ?? p.slug, label: p.title, detail: viaRedirect === null ? p.slug : `${viaRedirect}, now ${p.slug}` }));
      }
      // With nothing typed there's nothing to search for, and the page's own claims are the likely ones.
      if (query.trim() === "") {
        return (onPage ?? []).filter((c: WikiClaim) => c.status === "current").map((c) => ({ value: c.claimId, label: [c.subject, c.predicate, c.value].filter((x) => x !== null).join(" "), detail: c.claimId }));
      }
      const found = await engine.suggestClaims(vault, query);
      return found.ok ? found.value.map((c) => ({ value: c.id, label: c.label, detail: c.id })) : [];
    },
    [engine, vault, pages, onPage],
  );
};

interface FormProps {
  readonly vault: VaultId;
  /** Null for a page that doesn't exist yet. */
  readonly page: WikiPage | null;
  readonly initialTitle: string;
  readonly onReview: (changesetId: string | null, what: string, path: string) => void;
}

const EditorForm = ({ vault, page, initialTitle, onReview }: FormProps) => {
  const engine = useEngine();
  const go = useGo();
  const refresh = useRefreshWiki(vault);
  const resolver = useLinkResolver(vault);
  const keeper = useDraftKeeper(vault, page?.id ?? "new");
  const suggest = useSuggest(vault, page);

  // Read once. The base version stays what this edit started from, whatever a refetch brings, as that's what makes a stale save detectable.
  const [restored] = useState<Draft | null>(() => keeper.load());
  const [body, setBody] = useState(restored?.body ?? page?.body ?? "");
  const [title, setTitle] = useState(restored?.title ?? initialTitle);
  const [baseVersion, setBaseVersion] = useState(page === null ? 0 : (restored?.baseVersion ?? page.currentRevision));
  const [note, setNote] = useState("");
  const [editorKey, setEditorKey] = useState(0);
  const [saving, setSaving] = useState(false);
  const [problem, setProblem] = useState<readonly string[]>([]);
  const [conflict, setConflict] = useState<Conflict | null>(null);
  const [showRestored, setShowRestored] = useState(restored !== null);
  const [cantKeep, setCantKeep] = useState(false);

  const pristineBody = page?.body ?? "";
  const pristineTitle = page === null ? initialTitle : null;
  const standing = restored === null || page === null ? null : draftStanding(restored, page.currentRevision);
  const deferred = useDeferredValue(body);
  const claims = useClaimsFor(vault, deferred, page?.claims ?? []);
  const dirty = body !== pristineBody || (page === null && title.trim() !== "");

  const remember = (nextBody: string, nextTitle: string, base: number) => {
    keeper.edit({ body: nextBody, title: page === null ? nextTitle : null, baseVersion: page === null ? null : base, pristineBody, pristineTitle });
  };
  useEffect(() => {
    const timer = setInterval(() => setCantKeep(keeper.writeFailed()), 1000);
    return () => clearInterval(timer);
  }, [keeper]);

  const pageId = page?.id ?? null;
  const readPath = (id: string) => wikiPath(vault, { kind: "read", id, revision: null });

  const handle = async (outcome: SaveOutcome & { readonly pageId?: string }) => {
    setSaving(false);
    if (outcome.kind === "saved" || outcome.kind === "pending") {
      keeper.discard();
      await refresh();
      const id = outcome.pageId ?? pageId;
      const path = id === null ? wikiPath(vault, { kind: "list", view: "list", filter: "" }) : readPath(id);
      if (outcome.kind === "pending") onReview(outcome.changesetId, page === null ? "This new page" : "Your edit", path);
      go(path);
    } else if (outcome.kind === "conflict") {
      keeper.flush();
      setConflict(outcome);
    } else if (outcome.kind === "refused") {
      setProblem(outcome.reasons.length > 0 ? outcome.reasons : ["This was refused."]);
    } else {
      engine.reportFailure(outcome.failure, "action");
      setProblem([failureText(outcome.failure)]);
    }
  };

  const save = async (text: string, base: number) => {
    setSaving(true);
    setProblem([]);
    const rationale = note.trim() === "" ? DEFAULT_NOTE : note;
    if (pageId === null) await handle(await engine.createWikiPage(vault, { title: title.trim(), body: text, rationale }));
    else await handle(await engine.saveWikiPage(vault, pageId, { baseVersion: base, body: text, rationale }));
  };
  const requestSave = () => {
    if (saving) return;
    if (page === null && title.trim() === "") {
      setProblem(["A new page needs a title."]);
      return;
    }
    void save(body, baseVersion);
  };

  const discardDraft = () => {
    keeper.discard();
    setBody(pristineBody);
    setTitle(initialTitle);
    setBaseVersion(page?.currentRevision ?? 0);
    setEditorKey((k) => k + 1);
    setShowRestored(false);
    setProblem([]);
  };

  if (conflict !== null) {
    const adopt = (merged: string) => {
      setBody(merged);
      setBaseVersion(conflict.theirs.currentRevision);
      remember(merged, title, conflict.theirs.currentRevision);
      setEditorKey((k) => k + 1);
    };
    return (
      <ConflictView
        vault={vault}
        mine={conflict.mine}
        theirs={conflict.theirs}
        baseBody={conflict.base?.body ?? null}
        baseVersion={baseVersion}
        saving={saving}
        onSaveMerged={(merged) => {
          adopt(merged);
          setConflict(null);
          void save(merged, conflict.theirs.currentRevision);
        }}
        onEditMerged={(merged) => {
          adopt(merged);
          setConflict(null);
        }}
        onUseTheirs={() => {
          keeper.discard();
          void refresh().then(() => go(readPath(conflict.theirs.id)));
        }}
        onBack={() => setConflict(null)}
      />
    );
  }

  return (
    <section aria-label={page === null ? "New page" : `Editing ${page.title}`} className="flex flex-col gap-3">
      <div className="flex flex-wrap items-end justify-between gap-2">
        <h2 className="text-lg font-medium">{page === null ? "New page" : `Editing ${page.title}`}</h2>
        {page !== null && <span className="text-xs text-text-muted">From revision {baseVersion}</span>}
      </div>
      {page === null && <Input label="Title" value={title} onChange={(e) => { setTitle(e.target.value); remember(body, e.target.value, baseVersion); }} />}
      {showRestored && restored !== null && (
        <Callout tone="info" title={`Restored your unsaved draft from ${formatTime(restored.savedAt)}.`}>
          <Button variant="ghost" onClick={discardDraft}>Discard draft</Button>
        </Callout>
      )}
      {standing?.kind === "behind" && conflict === null && baseVersion === standing.baseVersion && (
        <Callout tone="warning" title={`This draft started from revision ${String(standing.baseVersion)}, and the page is now at ${String(standing.pageVersion)}.`}>
          Saving won't overwrite anything: you'll see both versions and choose.
        </Callout>
      )}
      {cantKeep && <Callout tone="warning" title="This browser can't keep a draft, so a reload would lose your text. Save soon." />}
      {problem.length > 0 && <Callout tone="danger" title={problem.join(" ")}>Your text is still here{cantKeep ? "." : ", and kept as a draft in this browser."}</Callout>}
      <div className="h-[32rem] min-h-0">
        <SplitPane
          label="Resize the editor and preview"
          start={
            <Suspense fallback={<Skeleton className="h-full w-full" />}>
              <MarkdownEditor key={editorKey} label="Page text in Markdown" initial={body} onChange={(text) => { setBody(text); remember(text, title, baseVersion); }} onSave={requestSave} suggest={suggest} />
            </Suspense>
          }
          end={
            <div data-testid="preview" aria-label="Preview">
              <PageMarkdown vault={vault} source={deferred} resolver={resolver} claims={claims} />
              <ClaimEvidence vault={vault} source={deferred} claims={claims} />
            </div>
          }
        />
      </div>
      <Input label="Why" placeholder={DEFAULT_NOTE} value={note} onChange={(e) => setNote(e.target.value)} />
      <div className="flex items-center gap-2">
        <Button variant="primary" disabled={saving || !dirty} onClick={requestSave}>{saving ? "Saving" : "Save"}</Button>
        <AppLink to={page === null ? wikiPath(vault, { kind: "list", view: "list", filter: "" }) : readPath(page.id)} className="inline-flex h-8 items-center px-3 text-sm text-text-muted hover:text-text">Close</AppLink>
        {dirty && restored === null && <Button variant="ghost" onClick={discardDraft}>Discard changes</Button>}
        <span className="text-xs text-text-muted">Ctrl or Cmd + S saves. Type [[ or {"{{"}claim: to link. Unsaved text stays in this browser.</span>
      </div>
    </section>
  );
};

export const NewPage = ({ vault, initialTitle, onReview }: { readonly vault: VaultId; readonly initialTitle: string; readonly onReview: FormProps["onReview"] }) => <EditorForm vault={vault} page={null} initialTitle={initialTitle} onReview={onReview} />;

/** The base version has to be fresh, so a cached read isn't enough to start from. */
export const EditPage = ({ vault, id, onReview }: { readonly vault: VaultId; readonly id: string; readonly onReview: FormProps["onReview"] }) => {
  const engine = useEngine();
  const started = useRef(false);
  const query = useQuery({ queryKey: QUERY_KEYS.wikiPage(vault, id, null), queryFn: async () => unwrap(await engine.getWikiPage(vault, id, null)), staleTime: 0, refetchOnMount: "always", refetchOnReconnect: false });
  if (!started.current && !query.isFetchedAfterMount && query.isFetching) return <Skeleton className="h-96 w-full" />;
  started.current = true;
  return <QueryView query={query} label="this page">{(page) => <EditorForm key={page.id} vault={vault} page={page} initialTitle="" onReview={onReview} />}</QueryView>;
};
