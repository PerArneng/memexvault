// The only package that names @ag-ui/client and rxjs. See docs/frontend-architecture.md. The console imports it lazily, so it loads when
// someone first sends a chat message.
import "./jitless";
import { HttpAgent } from "@ag-ui/client";
import type { HttpAgentFetchFn } from "@ag-ui/client";
import { agentEventFromWire } from "@memex/api";
import type { AgentChannel } from "@memex/core";
import type { ChannelEvent } from "@memex/types";

export interface AgUiOptions {
  /** Such as `/api/v1`. */
  readonly apiBaseUrl: string;
  /** For tests. The browser's own by default, which sends the session cookie to the same origin. */
  readonly fetch?: HttpAgentFetchFn;
}

/** The server wants this on anything a cookie would authorize, a run included. */
const CSRF_HEADERS = { "x-memex-csrf": "1" };

const isRecord = (value: unknown): value is Readonly<Record<string, unknown>> => typeof value === "object" && value !== null && !Array.isArray(value);

/** `HttpAgent` throws an `Error` with `status` and the parsed body as `payload` when the server refuses a run before streaming. */
const failureOf = (error: unknown): ChannelEvent => {
  const status = isRecord(error) && typeof error.status === "number" ? error.status : null;
  if (status !== null) {
    const payload = isRecord(error) && isRecord(error.payload) ? error.payload : {};
    const body = isRecord(payload.error) ? payload.error : {};
    return { kind: "refused", status, code: typeof body.code === "string" ? body.code : null, message: typeof body.message === "string" ? body.message : "" };
  }
  return { kind: "dropped", message: error instanceof Error ? error.message : "the connection failed" };
};

const isAbort = (error: unknown): boolean => isRecord(error) && error.name === "AbortError";

export const createAgUiAgentChannel = ({ apiBaseUrl, fetch }: AgUiOptions): AgentChannel => ({
  run: (request, listener) => {
    const agent = new HttpAgent({
      url: `${apiBaseUrl}/vaults/${encodeURIComponent(request.vault)}/chat/runs`,
      headers: CSRF_HEADERS,
      threadId: request.threadId,
      ...(fetch === undefined ? {} : { fetch }),
    });
    let done = false;
    const say = (event: ChannelEvent) => {
      if (!done) listener(event);
    };
    const close = () => {
      if (done) return;
      say({ kind: "closed" });
      done = true;
    };
    // `run` rather than `runAgent`: the transcript reducer in core keeps the state, and judges order itself, so the agent's own message
    // store and verifier would only be a second copy. Only the newest message goes, as the server reads nothing else and a long thread
    // would pass its body limit.
    const subscription = agent
      .run({
        threadId: request.threadId,
        runId: request.runId,
        messages: [{ id: request.messageId, role: "user", content: request.message }],
        tools: [],
        context: [],
        state: {},
        forwardedProps: {},
      })
      .subscribe({
        next: (event) => say({ kind: "event", event: agentEventFromWire(event) }),
        error: (error: unknown) => {
          // Stopped before the stream opened: the client only makes up its `abort` error once it's reading.
          say(isAbort(error) ? { kind: "event", event: { type: "run_error", message: "Request aborted", code: "abort" } } : failureOf(error));
          close();
        },
        complete: close,
      });
    return {
      stop: () => {
        if (done) return;
        // Closing the connection is the cancel on the server. The stop is said here and now, rather than when the client gets round to it.
        agent.abortRun();
        say({ kind: "event", event: { type: "run_error", message: "Request aborted", code: "abort" } });
        close();
        subscription.unsubscribe();
      },
    };
  },
});
