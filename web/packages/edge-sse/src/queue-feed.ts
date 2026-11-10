import { frameTouchesQueue } from "@memex/core";
import type { QueueFeed, QueueFeedEvent } from "@memex/core";

/** The fields of a durable frame this reads. Anything else in it is ignored. */
interface WireFrame {
  readonly kind?: unknown;
  readonly changesets?: unknown;
  readonly omitted?: unknown;
}

/** Reads a frame's data, and treats one it can't read as something that may have moved the queue, as reading again costs little and missing a change misleads. */
export const eventOfFrame = (data: string): QueueFeedEvent | null => {
  try {
    const frame = JSON.parse(data) as WireFrame;
    const changesets = Array.isArray(frame.changesets) ? frame.changesets.length : 0;
    const omitted = typeof frame.omitted === "number" ? frame.omitted : 0;
    return frameTouchesQueue({ changesets, omitted, kind: typeof frame.kind === "string" ? frame.kind : "" }) ? { kind: "changed" } : null;
  } catch {
    return { kind: "changed" };
  }
};

/** Follows `GET /activity`. `EventSource` resumes by itself with `Last-Event-ID`, and sends the session cookie because it's same-origin. */
export const createSseQueueFeed = ({ apiBaseUrl }: { readonly apiBaseUrl: string }): QueueFeed => ({
  follow: (vault, listener) => {
    const source = new EventSource(`${apiBaseUrl}/vaults/${encodeURIComponent(vault)}/activity`);
    source.addEventListener("hello", () => listener({ kind: "open" }));
    source.addEventListener("resync", () => listener({ kind: "resync" }));
    source.addEventListener("durable", (event) => {
      const next = eventOfFrame((event as MessageEvent<string>).data);
      if (next !== null) listener(next);
    });
    source.addEventListener("end", () => {
      source.close();
      listener({ kind: "closed" });
    });
    source.onerror = () => listener(source.readyState === EventSource.CLOSED ? { kind: "closed" } : { kind: "away" });
    return () => source.close();
  },
});
