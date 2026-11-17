import { actorText } from "@memex/core";
import type { VaultId } from "@memex/types";
import { Badge } from "@memex/ui";
import { useQuery } from "@tanstack/react-query";
import { Bot, CircleHelp, UserRound } from "lucide-react";
import { useEngine } from "./engine-context";
import { QUERY_KEYS, unwrap } from "./query";

/** A name changes rarely, so a screen asks once per actor for a few minutes, however many rows name them. */
const NAME_STALE_MS = 5 * 60_000;

/**
 * Who did something, by name, saying clearly whether it was a person or an agent. The name is rendered as text, never as markup.
 * While the name is read, or if it can't be, the id shows instead.
 */
export const ActorName = ({ vault, id }: { readonly vault: VaultId; readonly id: string }) => {
  const engine = useEngine();
  const query = useQuery({ queryKey: QUERY_KEYS.actor(vault, id), queryFn: async () => unwrap(await engine.getActor(vault, id)), staleTime: NAME_STALE_MS, retry: 1 });
  const text = actorText(id, query.data ?? null);
  const kind = query.data === undefined ? null : text.kind;
  return (
    <span data-actor={id} data-actor-kind={kind ?? "loading"} className="inline-flex flex-wrap items-center gap-1.5 text-xs text-text-muted" title={id}>
      {kind === "Agent" && <Badge tone="info"><Bot size={12} aria-hidden={true} /> Agent</Badge>}
      {kind === "Person" && <Badge><UserRound size={12} aria-hidden={true} /> Person</Badge>}
      {kind === "Unknown" && <Badge tone="warning"><CircleHelp size={12} aria-hidden={true} /> Unknown</Badge>}
      <span data-actor-name="" className="text-text">{text.name}</span>
      {text.note !== null && <span>({text.note})</span>}
    </span>
  );
};
