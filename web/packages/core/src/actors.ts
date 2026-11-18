import type { Actor, JobAction } from "@memex/types";

/** How an actor reads on screen. Every part is plain text, never markup, as a name is whatever its principal was called. */
export interface ActorText {
  /** The name, or a stand-in that says the principal isn't known. */
  readonly name: string;
  readonly kind: "Person" | "Agent" | "Unknown";
  /** Said beside the name, such as that the principal is disabled. */
  readonly note: string | null;
}

/** `null` while the name is still being read, or when it couldn't be: the id is then all there is. */
export const actorText = (id: string, actor: Actor | null): ActorText => {
  if (actor === null) return { name: id, kind: "Unknown", note: null };
  if (actor.kind === "unknown" || actor.name === null) return { name: `Unknown principal ${id}`, kind: "Unknown", note: "no longer exists, or never was a principal" };
  return { name: actor.name, kind: actor.kind === "agent" ? "Agent" : "Person", note: actor.disabled ? "disabled" : null };
};

/** "Cancelled by" or "Retried by", for a job's action. */
export const jobActionText = (action: JobAction): string => {
  if (action.action === "cancel") return "Cancelled by";
  if (action.action === "retry") return "Retried by";
  return `${action.action} by`;
};
