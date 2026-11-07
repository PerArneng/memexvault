import { isNotFound, parseWikiRoute, pageIndex, requestIdText, sectionPath, wikiPath } from "@memex/core";
import type { ApiFailure, PageIndex, SaveOutcome, WikiChange, WikiPageList, WikiRoute } from "@memex/core";
import type { VaultId, WikiAttribution as Attribution } from "@memex/types";
import { Badge, Button, Input } from "@memex/ui";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useRouter } from "@tanstack/react-router";
import { Bot, UserRound } from "lucide-react";
import { useCallback, useMemo, useState } from "react";
import type { MouseEvent, ReactNode } from "react";
import { useEngine } from "../engine-context";
import { QUERY_KEYS, unwrap } from "../query";

export const DEFAULT_NOTE = "Edited in the console";

export const LINK_BUTTON = "inline-flex h-8 items-center gap-1.5 rounded-md border border-border-strong bg-surface-overlay px-3 text-sm text-text hover:bg-surface-selected";
export const LINK_TEXT = "text-accent underline";

export const formatTime = (ms: number): string => `${new Date(ms).toISOString().slice(0, 16).replace("T", " ")} UTC`;

/** A same-site link that moves without a reload. It isn't the router's `Link`, which marks itself as active, because a page's rendered links must come out the same in the reader and the editor's preview. */
export const useGo = () => {
  const router = useRouter();
  return useCallback((to: string, replace = false) => {
    if (replace) router.history.replace(to);
    else router.history.push(to);
  }, [router]);
};

export const AppLink = ({ to, className = LINK_TEXT, children, ...rest }: { readonly to: string; readonly className?: string; readonly children: ReactNode; readonly title?: string; readonly "data-link-state"?: string }) => {
  const go = useGo();
  const onClick = (e: MouseEvent<HTMLAnchorElement>) => {
    if (e.metaKey || e.ctrlKey || e.shiftKey || e.altKey || e.button !== 0) return;
    e.preventDefault();
    go(to);
  };
  return <a href={to} onClick={onClick} className={className} {...rest}>{children}</a>;
};

export const routeFrom = (rest: readonly string[], search: string): WikiRoute => {
  const query = new URLSearchParams(search);
  return parseWikiRoute(rest, (name) => query.get(name));
};

export const useWikiPages = (vault: VaultId) => {
  const engine = useEngine();
  return useQuery({ queryKey: QUERY_KEYS.wikiPages(vault), queryFn: async () => unwrap(await engine.listWikiPages(vault)) });
};

export interface LinkResolver {
  readonly index: PageIndex;
  /** The list may be cut short, so a page that isn't in it may still exist. */
  readonly complete: boolean;
}

const EMPTY: WikiPageList = { pages: [], truncated: true };

export const useLinkResolver = (vault: VaultId): LinkResolver => {
  const { data = EMPTY } = useWikiPages(vault);
  return useMemo(() => ({ index: pageIndex(data.pages), complete: !data.truncated }), [data]);
};

/** After a write, what's cached about this vault's pages is out of date. A read not on screen goes, so opening it can't show the old text first, and one on screen refetches. */
export const useRefreshWiki = (vault: VaultId) => {
  const queryClient = useQueryClient();
  return useCallback(async () => {
    queryClient.removeQueries({ queryKey: [...QUERY_KEYS.wiki(vault), "page"], type: "inactive" });
    await queryClient.invalidateQueries({ queryKey: QUERY_KEYS.wiki(vault) });
    await queryClient.invalidateQueries({ queryKey: QUERY_KEYS.objects(vault) });
  }, [queryClient, vault]);
};

const AuthorBadge = ({ attribution }: { readonly attribution: Attribution }) =>
  attribution.machine ? (
    <Badge tone="info"><Bot size={12} aria-hidden={true} /> Agent</Badge>
  ) : (
    <Badge><UserRound size={12} aria-hidden={true} /> Person</Badge>
  );

/** Who made a change, clearly saying whether it was a person or an agent. */
export const ChangeAuthor = ({ vault, changesetId, fallback }: { readonly vault: VaultId; readonly changesetId: string; readonly fallback: string }) => {
  const engine = useEngine();
  const query = useQuery({ queryKey: QUERY_KEYS.wikiAttribution(vault, changesetId), queryFn: async () => unwrap(await engine.getWikiAttribution(vault, changesetId)), staleTime: Infinity });
  if (query.data === undefined) return <span className="text-xs text-text-muted">{fallback}</span>;
  const { proposer, extractor, rationale } = query.data;
  return (
    <span className="inline-flex flex-wrap items-center gap-1.5 text-xs text-text-muted" title={rationale}>
      <AuthorBadge attribution={query.data} />
      <span className="text-text">{proposer}</span>
      {extractor !== null && extractor !== proposer && <span>via {extractor}</span>}
    </span>
  );
};

export const failureText = (failure: ApiFailure): string => {
  if (failure.kind === "unreachable") return "Couldn't reach the server, so nothing was changed that this page can confirm. Try again.";
  const id = requestIdText(failure);
  return id === null ? failure.message : `${failure.message} (${id})`;
};

export const Callout = ({ tone, title, children }: { readonly tone: "info" | "warning" | "danger"; readonly title: string; readonly children?: ReactNode }) => {
  const box = { info: "border-info bg-info-surface", warning: "border-warning bg-warning-surface", danger: "border-danger bg-danger-surface" }[tone];
  const text = { info: "text-info", warning: "text-warning", danger: "text-danger" }[tone];
  return (
    <div role={tone === "info" ? "status" : "alert"} className={`flex flex-col gap-1 rounded-lg border p-3 ${box}`}>
      <p className={`font-medium ${text}`}>{title}</p>
      {children !== undefined && <div className="text-sm text-text-muted">{children}</div>}
    </div>
  );
};

/** What to tell a person about a write that went through or was held. */
export const ReviewNotice = ({ vault, changesetId, what }: { readonly vault: VaultId; readonly changesetId: string | null; readonly what: string }) => (
  <Callout tone="info" title={`${what} is waiting for review.`}>
    The page doesn't change until it's approved. <AppLink to={sectionPath(vault, "review", changesetId ?? undefined)}>Open it in Review</AppLink>
  </Callout>
);

export interface InlineActionProps {
  readonly title: string;
  readonly confirmLabel: string;
  readonly danger?: boolean;
  readonly description?: string;
  readonly children?: ReactNode;
  readonly disabled?: boolean;
  readonly defaultNote: string;
  readonly run: (note: string) => Promise<WikiChange | SaveOutcome>;
  readonly onDone: (change: WikiChange | SaveOutcome) => void;
  readonly onCancel: () => void;
}

/** A confirmation in the page rather than a modal, because Radix's modal logs a CSP error per opening. See spec 040. */
export const InlineAction = ({ title, confirmLabel, danger = false, description, children, disabled = false, defaultNote, run, onDone, onCancel }: InlineActionProps) => {
  const [note, setNote] = useState(defaultNote);
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const confirm = async () => {
    setBusy(true);
    setProblem(null);
    const outcome = await run(note.trim() === "" ? defaultNote : note);
    setBusy(false);
    if (outcome.kind === "failed") setProblem(failureText(outcome.failure));
    else if (outcome.kind === "refused") setProblem(outcome.reasons.length > 0 ? outcome.reasons.join(" ") : "This was refused.");
    else if (outcome.kind === "stale") setProblem("The page has changed since this was opened, so nothing was done. It has been reloaded: look it over and try again.");
    if (outcome.kind !== "failed" && outcome.kind !== "refused") onDone(outcome);
  };
  return (
    <section aria-label={title} className="flex flex-col gap-2 rounded-lg border border-border-strong bg-surface-raised p-3">
      <h3 className="font-medium">{title}</h3>
      {description !== undefined && <p className="text-sm text-text-muted">{description}</p>}
      {children}
      <Input label="Why" value={note} onChange={(e) => setNote(e.target.value)} />
      {problem !== null && <Callout tone="danger" title={problem} />}
      <div className="flex gap-2">
        <Button variant={danger ? "danger" : "primary"} disabled={busy || disabled} onClick={() => void confirm()}>{confirmLabel}</Button>
        <Button variant="ghost" onClick={onCancel}>Cancel</Button>
      </div>
    </section>
  );
};

export const downloadText = (name: string, text: string): void => {
  const url = URL.createObjectURL(new Blob([text], { type: "text/markdown;charset=utf-8" }));
  const link = document.createElement("a");
  link.href = url;
  link.download = name;
  link.click();
  URL.revokeObjectURL(url);
};

export { isNotFound, wikiPath };
