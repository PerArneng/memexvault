import { TRUST_TIERS } from "@memex/core";
import type { DecisionOutcome } from "@memex/core";
import { Badge, Button, EpistemicBadge, TrustBadge } from "@memex/ui";
import type { EpistemicStatus, TrustTier } from "@memex/ui";
import { useQueryClient } from "@tanstack/react-query";
import { Bot, UserRound } from "lucide-react";
import { useCallback, useId, useState } from "react";
import type { KeyboardEvent } from "react";
import { QUERY_KEYS } from "../query";
import { failureText } from "../wiki/wiki-common";

export const useRefreshReview = (vault: string) => {
  const queryClient = useQueryClient();
  return useCallback(() => queryClient.invalidateQueries({ queryKey: QUERY_KEYS.review(vault) }), [queryClient, vault]);
};

export const WhoBadge = ({ machine }: { readonly machine: boolean }) =>
  machine ? <Badge tone="info"><Bot size={12} aria-hidden={true} /> Agent</Badge> : <Badge><UserRound size={12} aria-hidden={true} /> Person</Badge>;

const EPISTEMIC: readonly string[] = ["verified", "asserted", "inferred", "disputed", "superseded"];

export const TrustOf = ({ tier }: { readonly tier: string | null }) =>
  tier !== null && (TRUST_TIERS as readonly string[]).includes(tier) ? <TrustBadge tier={tier as TrustTier} /> : <Badge tone="warning">No known trust tier</Badge>;

export const EpistemicOf = ({ status }: { readonly status: string | null }) =>
  status !== null && EPISTEMIC.includes(status) ? <EpistemicBadge status={status as EpistemicStatus} /> : status === null ? null : <Badge>{status}</Badge>;

export const STATE_TONE: Readonly<Record<string, "neutral" | "success" | "danger">> = { pending: "neutral", committed: "success", rejected: "danger" };
export const StateBadge = ({ state }: { readonly state: string }) => <Badge tone={STATE_TONE[state] ?? "neutral"}>{state}</Badge>;

/** The words shown for a decision that didn't go through. A conflict's message comes from the server and names who decided. */
export const outcomeNotice = (outcome: Exclude<DecisionOutcome, { readonly kind: "done" }>): { readonly title: string; readonly detail: string } => {
  switch (outcome.kind) {
    case "conflict":
      return { title: "This couldn't be done, and nothing changed.", detail: outcome.message };
    case "not_allowed":
      return { title: "You aren't allowed to do this.", detail: outcome.message };
    case "invalid":
      return { title: "The server refused this.", detail: outcome.message };
    case "failed":
      return { title: "It may not have gone through.", detail: failureText(outcome.failure) };
  }
};

export interface ReasonFormProps {
  readonly label: string;
  readonly confirmLabel: string;
  /** Must be filled in before it can go. */
  readonly required: boolean;
  readonly initial?: string;
  readonly danger?: boolean;
  readonly description?: string;
  readonly problem?: string | null;
  readonly busy?: boolean;
  readonly onSubmit: (reason: string) => void;
  readonly onCancel: () => void;
}

/** In the page, not in a modal, while the modal's CSP error is undecided. Enter sends, Shift+Enter is a new line and Esc cancels. */
export const ReasonForm = ({ label, confirmLabel, required, initial = "", danger = false, description, problem = null, busy = false, onSubmit, onCancel }: ReasonFormProps) => {
  const id = useId();
  const [reason, setReason] = useState(initial);
  const [tried, setTried] = useState(false);
  const missing = required && reason.trim() === "";
  const send = () => {
    setTried(true);
    if (!missing && !busy) onSubmit(reason.trim());
  };
  const onKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      send();
    }
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      onCancel();
    }
  };
  const shown = problem ?? (tried && missing ? "This needs a reason." : null);
  return (
    <div className="flex flex-col gap-2 rounded-lg border border-border-strong bg-surface-raised p-3" data-testid="reason-form">
      {description !== undefined && <p className="text-sm text-text-muted">{description}</p>}
      <label htmlFor={id} className="text-sm">{label}</label>
      <textarea
        id={id}
        // The panel opens because someone asked for it by key, so typing goes straight here.
        autoFocus={true}
        rows={2}
        value={reason}
        onChange={(e) => setReason(e.target.value)}
        onKeyDown={onKeyDown}
        aria-invalid={shown !== null}
        className="rounded-md border border-border-strong bg-surface px-2 py-1 text-sm text-text"
      />
      {shown !== null && <p role="alert" className="text-sm text-danger">{shown}</p>}
      <div className="flex items-center gap-2">
        <Button variant={danger ? "danger" : "primary"} disabled={busy} onClick={send}>{confirmLabel}</Button>
        <Button onClick={onCancel}>Cancel</Button>
        <span className="text-xs text-text-muted">Enter to send, Esc to cancel</span>
      </div>
    </div>
  );
};
