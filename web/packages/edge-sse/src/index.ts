// See docs/frontend-architecture.md.
export { createSseQueueFeed, eventOfFrame } from "./queue-feed";
import { activityFrameFromWire, failureFromWire } from "@memex/api";
import type { ActivityFeed, FeedEvent } from "@memex/core";
import type { ApiFailure, VaultId } from "@memex/types";
import { createSseParser } from "./sse-parser";

export { createSseParser } from "./sse-parser";
export type { SseMessage } from "./sse-parser";

const FIRST_RETRY_MS = 500;
const MAX_RETRY_MS = 15_000;
/** Heartbeats come every 15 s, so three missed means a connection that went without saying so. */
const SILENCE_LIMIT_MS = 45_000;
const TOO_MANY_RETRY_MS = 15_000;

/** These won't get better by trying again: the session, the grant or the vault is gone. */
const FINAL_ENDS: readonly string[] = ["auth_ended", "scope_lost", "vault_gone"];
const FINAL_STATUSES: readonly number[] = [401, 403, 404];

const pause = (ms: number, signal: AbortSignal) =>
  new Promise<void>((done) => {
    if (signal.aborted) {
      done();
      return;
    }
    const timer = setTimeout(done, ms);
    signal.addEventListener("abort", () => {
      clearTimeout(timer);
      done();
    }, { once: true });
  });

const failureOf = async (res: Response): Promise<ApiFailure> => {
  const body: unknown = await res.json().catch(() => null);
  return failureFromWire(res.status, body, res.headers);
};

/**
 * `fetch` rather than `EventSource`, which hides a refusal's status and so can't tell an ended session from a lost connection, and can't notice a connection that
 * has silently died. It's open to any screen: each `follow` is one stream, and a principal may hold 16 of them.
 */
export const createSseActivityFeed = ({ apiBaseUrl }: { readonly apiBaseUrl: string }): ActivityFeed => ({
  follow: (vault: VaultId, after: number, listener: (event: FeedEvent) => void) => {
    const stop = new AbortController();
    let position = after;

    /** Reads one connection until it ends. Says whether to try again, and any wait that needs. */
    const connection = async (): Promise<{ readonly again: boolean; readonly wait: number; readonly heard: boolean }> => {
      const attempt = new AbortController();
      const stopAttempt = () => attempt.abort();
      stop.signal.addEventListener("abort", stopAttempt, { once: true });
      let heard = false;
      let ended: string | null = null;
      let silence = setTimeout(stopAttempt, SILENCE_LIMIT_MS);
      try {
        const res = await fetch(`${apiBaseUrl}/vaults/${encodeURIComponent(vault)}/activity?after=${String(position)}`, {
          headers: { accept: "text/event-stream", "last-event-id": String(position) },
          signal: attempt.signal,
          cache: "no-store",
        });
        if (!res.ok || res.body === null) {
          const failure = await failureOf(res);
          if (FINAL_STATUSES.includes(res.status)) {
            listener({ type: "ended", reason: res.status === 401 ? "auth_ended" : "scope_lost", failure });
            return { again: false, wait: 0, heard };
          }
          return { again: true, wait: res.status === 429 ? TOO_MANY_RETRY_MS : 0, heard };
        }
        const parser = createSseParser((message) => {
          if (message.id !== null) {
            const id = Number(message.id);
            if (Number.isSafeInteger(id)) position = id;
          }
          const frame = activityFrameFromWire(message.event, message.data);
          if (frame.kind === "end") {
            ended = frame.reason;
            return;
          }
          heard = true;
          listener({ type: "frame", frame });
        });
        const reader = res.body.pipeThrough(new TextDecoderStream()).getReader();
        for (;;) {
          const { done, value } = await reader.read();
          clearTimeout(silence);
          if (done) break;
          silence = setTimeout(stopAttempt, SILENCE_LIMIT_MS);
          parser.feed(value);
          if (ended !== null) {
            void reader.cancel();
            break;
          }
        }
      } catch {
        // A lost connection and a silent one that was given up on read the same from here.
      } finally {
        clearTimeout(silence);
        stop.signal.removeEventListener("abort", stopAttempt);
      }
      if (ended !== null && FINAL_ENDS.includes(ended)) {
        listener({ type: "ended", reason: ended, failure: null });
        return { again: false, wait: 0, heard };
      }
      return { again: true, wait: 0, heard };
    };

    const run = async () => {
      let backoff = FIRST_RETRY_MS;
      while (!stop.signal.aborted) {
        const { again, wait, heard } = await connection();
        if (!again || stop.signal.aborted) return;
        listener({ type: "lost" });
        if (heard) backoff = FIRST_RETRY_MS;
        await pause(Math.max(wait, backoff), stop.signal);
        backoff = Math.min(backoff * 2, MAX_RETRY_MS);
      }
    };
    void run();
    return () => stop.abort();
  },
});
