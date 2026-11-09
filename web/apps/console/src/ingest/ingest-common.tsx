import type { ActivityWatch, BoardDocument, Connection } from "@memex/core";
import type { VaultId } from "@memex/types";
import { Badge, StageBadge } from "@memex/ui";
import type { JobStage, Tone } from "@memex/ui";
import { useEffect, useState } from "react";
import { useEngine } from "../engine-context";

/** How often arrived frames are applied and drawn, so a burst of events can't block input. */
const RENDER_MS = 200;
const CLOCK_MS = 1000;

/** One stream and one picture for the whole screen, so moving between its views doesn't reopen anything. */
export const useActivityWatch = (vault: VaultId): ActivityWatch | null => {
  const engine = useEngine();
  const [watch, setWatch] = useState<ActivityWatch | null>(null);
  useEffect(() => {
    const created = engine.watchActivity(vault);
    setWatch(created);
    const timer = setInterval(() => created.tick(), RENDER_MS);
    return () => {
      clearInterval(timer);
      created.stop();
    };
  }, [engine, vault]);
  return watch;
};

/** The watch's picture. It changes at most once a tick, and once a second so that times in stage move on. */
export const useActivityState = (watch: ActivityWatch) => {
  const [, setVersion] = useState(0);
  useEffect(() => {
    const bump = () => setVersion((v) => v + 1);
    const unsubscribe = watch.subscribe(bump);
    const timer = setInterval(bump, CLOCK_MS);
    bump();
    return () => {
      unsubscribe();
      clearInterval(timer);
    };
  }, [watch]);
  return { state: watch.state(), now: watch.now() };
};

const CONNECTION: Readonly<Record<Connection["status"], { readonly tone: Tone; readonly label: string }>> = {
  connecting: { tone: "neutral", label: "Connecting" },
  live: { tone: "success", label: "Live" },
  reconnecting: { tone: "warning", label: "Reconnecting" },
  ended: { tone: "danger", label: "Not live" },
};

const END_TEXT: Readonly<Record<string, string>> = {
  auth_ended: "Your session or token has ended, so this view has stopped updating.",
  scope_lost: "Your token can't read this vault any more, so this view has stopped updating.",
  vault_gone: "This vault is gone.",
};

export const connectionDetail = (connection: Connection): string | null => {
  switch (connection.status) {
    case "reconnecting":
      return "The live connection dropped. It's reconnecting by itself, and what's below is what was known when it dropped.";
    case "ended":
      return END_TEXT[connection.reason] ?? "The server ended the live stream.";
    default:
      return null;
  }
};

export const ConnectionBadge = ({ connection }: { readonly connection: Connection }) => {
  const shown = CONNECTION[connection.status];
  return (
    <span role="status" data-connection={connection.status}>
      <span className="sr-only">Live connection: </span>
      <Badge tone={shown.tone}>{shown.label}</Badge>
    </span>
  );
};

const STATUS_BADGE: Readonly<Partial<Record<string, { readonly stage: JobStage; readonly label: string }>>> = {
  queued: { stage: "waiting", label: "Queued" },
  waiting: { stage: "waiting", label: "Waiting" },
  retrying: { stage: "running", label: "Retrying" },
  active: { stage: "running", label: "Running" },
  partial: { stage: "partial", label: "Partial" },
  failed: { stage: "failed", label: "Failed" },
  done: { stage: "done", label: "Done" },
};

export const statusLabel = (status: string): string => STATUS_BADGE[status]?.label ?? status;

/** A status from a newer server still shows, as what it says. */
export const StatusOf = ({ doc }: { readonly doc: Pick<BoardDocument, "status" | "stage"> }) => {
  const known = STATUS_BADGE[doc.status];
  if (known === undefined) return <Badge>{doc.status}</Badge>;
  if (doc.status === "waiting" && (doc.stage === "extraction" || doc.stage === "review")) return <StageBadge stage={known.stage} detail="for the LLM" />;
  // The badge's own word covers `waiting`, `running`, `partial`, `failed` and `done`, and the rest say more.
  return known.label === "Queued" || known.label === "Retrying" ? <StageBadge stage={known.stage} detail={known.label.toLowerCase()} /> : <StageBadge stage={known.stage} />;
};

export const ProgressBar = ({ label, value, max }: { readonly label: string; readonly value: number; readonly max: number }) => {
  const share = max > 0 ? Math.min(1, value / max) : 0;
  return (
    <div role="progressbar" aria-label={label} aria-valuemin={0} aria-valuemax={max} aria-valuenow={value} className="h-1.5 w-full overflow-hidden rounded-sm bg-surface-overlay">
      <div className="h-full bg-accent" style={{ width: `${String(share * 100)}%` }} />
    </div>
  );
};
