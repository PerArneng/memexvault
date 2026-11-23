import { prepareReply, stepSummary, unresolvedCount } from "@memex/core";
import type { CitationIndex, ReplyEnd, TranscriptEntry } from "@memex/core";
import type { Identity, VaultId } from "@memex/types";
import { Badge } from "@memex/ui";
import { SafeMarkdown } from "@memex/ui/markdown";
import { AlertTriangle, BookOpen, CircleSlash, FileSearch, GitPullRequest, Loader2, Search, Wrench } from "lucide-react";
import { useCallback, useMemo } from "react";
import { AppLink } from "../wiki/wiki-common";
import { ProposalCard } from "./proposal-card";

type Reply = Extract<TranscriptEntry, { kind: "reply" }>;
type Step = Extract<TranscriptEntry, { kind: "step" }>;

const END_TEXT: Readonly<Record<Exclude<ReplyEnd, "done" | "streaming">, string>> = {
  stopped: "Stopped: this reply is partial",
  cut: "Cut short at its limit",
  budget: "Ended by the token budget",
  failed: "Failed: this reply is partial",
  dropped: "Connection lost: this reply may be partial",
};

const CitationChip = ({ id, path, n, insideLink }: { readonly id: string; readonly path: string | null; readonly n: number; readonly insideLink: boolean }) => {
  const label = `[${String(n)}]`;
  if (path === null) {
    return (
      <span data-citation-unresolved={id} title={`Cites ${id}, which nothing in this thread returned, so it can't be checked`} className="mx-0.5 inline-flex items-baseline rounded-sm border border-dashed border-danger px-1 font-mono text-xs text-danger line-through">
        {label}
        <span className="sr-only"> (citation that doesn't resolve)</span>
      </span>
    );
  }
  if (insideLink) return <span data-citation={id} className="font-mono text-xs">{label}</span>;
  return (
    <AppLink to={path} title={`Open the source of ${id}`} data-link-state="citation" className="mx-0.5 inline-flex items-baseline rounded-sm bg-info-surface px-1 font-mono text-xs text-info no-underline hover:underline">
      <span data-citation={id}>{label}</span>
      <span className="sr-only"> (open the cited source)</span>
    </AppLink>
  );
};

const ReplyView = ({ reply, index }: { readonly reply: Reply; readonly index: CitationIndex }) => {
  const prepared = useMemo(() => prepareReply(reply.text, index), [reply.text, index]);
  const renderInline = useCallback(
    (i: number, insideLink: boolean) => {
      const cite = prepared.cites[i];
      return cite === undefined ? null : <CitationChip id={cite.id} path={cite.path} n={i + 1} insideLink={insideLink} />;
    },
    [prepared],
  );
  const rawInline = useCallback((i: number) => {
    const cite = prepared.cites[i];
    return cite === undefined ? "" : `[cite:${cite.id}]`;
  }, [prepared]);
  const unresolved = unresolvedCount(prepared.cites);
  const ended = reply.end === "done" || reply.end === "streaming" ? null : END_TEXT[reply.end];
  return (
    <article data-reply={reply.id} data-reply-end={reply.end} className={`flex flex-col gap-1.5 rounded-lg px-3 py-2 ${ended === null ? "bg-surface" : "border border-dashed border-warning bg-surface"}`}>
      <div className="min-w-0 break-words text-sm">
        {reply.text === "" && reply.end === "streaming" ? <span className="text-text-muted">…</span> : <SafeMarkdown source={prepared.text} renderInline={renderInline} rawInline={rawInline} />}
        {reply.end === "streaming" && <span aria-hidden={true} className="ml-0.5 inline-block h-3 w-1.5 animate-pulse bg-text-muted align-middle" />}
      </div>
      {(ended !== null || unresolved > 0) && (
        <p className="flex flex-wrap items-center gap-1.5 text-xs">
          {ended !== null && <Badge tone="warning"><CircleSlash size={12} aria-hidden={true} /> {ended}</Badge>}
          {unresolved > 0 && <Badge tone="danger"><AlertTriangle size={12} aria-hidden={true} /> {unresolved === 1 ? "1 citation doesn't resolve" : `${String(unresolved)} citations don't resolve`}</Badge>}
        </p>
      )}
    </article>
  );
};

const STEP_ICONS = { search: Search, read: BookOpen, provenance: FileSearch, propose: GitPullRequest } as const;

const StepView = ({ step, identity, vault }: { readonly step: Step; readonly identity: Identity; readonly vault: VaultId }) => {
  const Icon = step.state === "running" ? Loader2 : (STEP_ICONS[step.name as keyof typeof STEP_ICONS] ?? Wrench);
  const answer = step.answer;
  return (
    <div data-step={step.id} data-step-name={step.name} data-step-state={step.state} className="flex flex-col gap-1.5 text-xs text-text-muted">
      <span className="flex items-center gap-1.5">
        <Icon size={13} aria-hidden={true} className={step.state === "running" ? "animate-spin" : ""} />
        <span className="min-w-0 break-words">{stepSummary(step)}</span>
        {step.state === "failed" && <Badge tone="danger">failed</Badge>}
      </span>
      {answer?.kind === "error" && <span className="pl-5 text-danger">{answer.message}</span>}
      {answer?.kind === "search" && answer.items.length > 0 && (
        <details className="pl-5">
          <summary className="cursor-pointer">What it found</summary>
          <ul className="mt-1 flex flex-col gap-0.5">
            {answer.items.slice(0, 10).map((item) => (
              <li key={item.id} className="truncate"><Badge>{item.kind}</Badge> {item.heading ?? item.text}</li>
            ))}
          </ul>
        </details>
      )}
      {answer?.kind === "proposal" && <ProposalCard identity={identity} vault={vault} proposalId={answer.proposalId} refused={answer.refused} />}
    </div>
  );
};

export interface TranscriptViewProps {
  readonly identity: Identity;
  readonly vault: VaultId;
  readonly entries: readonly TranscriptEntry[];
  readonly index: CitationIndex;
}

/** The thread, as it streams. Everything the model or a tool said is shown as text or through `SafeMarkdown`, never as markup. */
export const TranscriptView = ({ identity, vault, entries, index }: TranscriptViewProps) => (
  <div role="log" aria-label="Conversation" aria-live="polite">
    <ol className="flex flex-col gap-3">
      {entries.map((e) => (
        <li key={`${e.kind}-${e.id}`}>
          {e.kind === "user" ? (
            <p data-user-message={e.id} className="ml-8 whitespace-pre-wrap break-words rounded-lg bg-surface-selected px-3 py-2 text-sm">{e.text}</p>
          ) : e.kind === "reply" ? (
            <ReplyView reply={e} index={index} />
          ) : e.kind === "step" ? (
            <StepView step={e} identity={identity} vault={vault} />
          ) : (
            <p role={e.tone === "error" ? "alert" : "status"} data-notice={e.tone} className={`text-xs ${e.tone === "error" ? "text-danger" : e.tone === "stopped" ? "text-warning" : "text-text-muted"}`}>{e.text}</p>
          )}
        </li>
      ))}
    </ol>
  </div>
);
