import { canChangeSource, scopeIn, trustEffect, TRUST_TIERS } from "@memex/core";
import type { SourceTrustOutcome, TrustEffect } from "@memex/core";
import { sectionPath } from "@memex/core";
import type { DocumentSource, Identity, VaultId } from "@memex/types";
import { Button, Input } from "@memex/ui";
import { useQueryClient } from "@tanstack/react-query";
import { useState } from "react";
import { useEngine } from "../engine-context";
import { QUERY_KEYS } from "../query";
import { TierOf } from "../viewer/badges";
import { AppLink, Callout, failureText } from "../wiki/wiki-common";

const RULE = "Only sources a person marked trusted or verified can auto-commit an agent's claims. Everything else waits for a person, however good the agent's record.";

const EFFECT_TEXT: Readonly<Record<TrustEffect, string>> = {
  same: "This is the tier it already has.",
  review_always: "Raising an untrusted source always waits for a person to approve it in Review, so the tier won't change until then.",
  person_commits_agent_waits: "From a person this applies at once, and agents' claims from this source can then auto-commit. From an agent it waits for a person to approve it.",
  lowers: "From a person this applies at once. From an agent it waits for approval unless that agent has earned auto-commit. Claims already committed stay as they are.",
};

const outcomeText = (outcome: SourceTrustOutcome): { readonly tone: "info" | "danger"; readonly title: string } | null => {
  switch (outcome.kind) {
    case "saved":
      return null;
    case "pending":
      return { tone: "info", title: "This change is waiting for review, and the tier hasn't changed yet." };
    case "refused":
      return { tone: "danger", title: outcome.reasons.join(" ") || "This change was refused." };
    case "stale":
      return { tone: "danger", title: "This source has changed since it was opened, so nothing was done. It has been reloaded: look it over and try again." };
    case "failed":
      return { tone: "danger", title: failureText(outcome.failure) };
  }
};

/** In the page rather than a modal, as Radix's modal logs a CSP error per opening. See spec 040. */
export const TrustTierPanel = ({ vault, source, identity, onClose }: { readonly vault: VaultId; readonly source: DocumentSource; readonly identity: Identity; readonly onClose: () => void }) => {
  const engine = useEngine();
  const queryClient = useQueryClient();
  const [tier, setTier] = useState(source.trustTier);
  const [note, setNote] = useState("");
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<{ readonly outcome: SourceTrustOutcome } | null>(null);
  const message = result === null ? null : outcomeText(result.outcome);
  const changesetId = result !== null && result.outcome.kind === "pending" ? result.outcome.changesetId : null;
  const allowed = canChangeSource(identity, vault);

  const apply = async () => {
    setBusy(true);
    const outcome = await engine.setSourceTrust(vault, source.id, { baseVersion: source.version, tier, rationale: note.trim() === "" ? `Set the trust tier to ${tier} in the console` : note });
    setBusy(false);
    setResult({ outcome });
    await queryClient.invalidateQueries({ queryKey: QUERY_KEYS.documents(vault) });
    if (outcome.kind === "saved") onClose();
  };

  return (
    <section aria-label="Change trust tier" className="flex flex-col gap-2 rounded-lg border border-border-strong bg-surface-raised p-3">
      <h2 className="font-medium">Change trust tier</h2>
      <p className="text-sm text-text-muted">{RULE}</p>
      {!allowed ? (
        <Callout tone="warning" title="You can't change this source's trust tier.">
          It needs <strong>propose</strong> scope, and your token has <strong>{scopeIn(identity, vault) ?? "none"}</strong>.
        </Callout>
      ) : (
        <>
          <fieldset className="flex flex-wrap gap-3">
            <legend className="sr-only">New trust tier</legend>
            {TRUST_TIERS.map((option) => (
              <label key={option} className="inline-flex items-center gap-1.5">
                <input type="radio" name="trust-tier" value={option} checked={tier === option} onChange={() => setTier(option)} />
                <TierOf tier={option} />
              </label>
            ))}
          </fieldset>
          <p data-trust-effect={trustEffect(source.trustTier, tier)} className="text-sm text-text-muted">{EFFECT_TEXT[trustEffect(source.trustTier, tier)]}</p>
          <Input label="Why" value={note} onChange={(e) => setNote(e.target.value)} />
        </>
      )}
      {message !== null && (
        <Callout tone={message.tone} title={message.title}>
          {changesetId !== null && <AppLink to={sectionPath(vault, "review", changesetId)}>Open it in Review</AppLink>}
        </Callout>
      )}
      <div className="flex gap-2">
        {allowed && <Button variant="primary" disabled={busy || tier === source.trustTier || result?.outcome.kind === "pending"} onClick={() => void apply()}>Change to {tier}</Button>}
        <Button variant="ghost" onClick={onClose}>Close</Button>
      </div>
    </section>
  );
};

export const ReprocessPanel = ({ vault, source, identity, onClose }: { readonly vault: VaultId; readonly source: DocumentSource; readonly identity: Identity; readonly onClose: () => void }) => {
  const engine = useEngine();
  const queryClient = useQueryClient();
  const [busy, setBusy] = useState(false);
  const [state, setState] = useState<{ readonly done: boolean; readonly problem: string | null }>({ done: false, problem: null });
  const allowed = canChangeSource(identity, vault);
  const run = async () => {
    setBusy(true);
    const result = await engine.reprocessSource(vault, source.id);
    setBusy(false);
    setState(result.ok ? { done: true, problem: null } : { done: false, problem: failureText(result.failure) });
    await queryClient.invalidateQueries({ queryKey: QUERY_KEYS.renditions(vault, source.id) });
  };
  return (
    <section aria-label="Reprocess" className="flex flex-col gap-2 rounded-lg border border-border-strong bg-surface-raised p-3">
      <h2 className="font-medium">Reprocess this source</h2>
      <p className="text-sm text-text-muted">This reads the original again and makes a new rendition. The old one stays readable, and claims move to the new one where their quote can still be found. Any that can't be found are flagged for review.</p>
      {!allowed && (
        <Callout tone="warning" title="You can't reprocess this source.">
          It needs <strong>propose</strong> scope, and your token has <strong>{scopeIn(identity, vault) ?? "none"}</strong>.
        </Callout>
      )}
      {state.done && <Callout tone="info" title="Reprocessing has been queued. The new rendition appears under Renditions when it's done." />}
      {state.problem !== null && <Callout tone="danger" title={state.problem} />}
      <div className="flex gap-2">
        {allowed && !state.done && <Button variant="primary" disabled={busy} onClick={() => void run()}>Reprocess</Button>}
        <Button variant="ghost" onClick={onClose}>Close</Button>
      </div>
    </section>
  );
};
