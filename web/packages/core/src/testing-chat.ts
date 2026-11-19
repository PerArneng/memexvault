import type { AgentEvent, ChannelEvent } from "@memex/types";
import type { AgentChannel, AgentRunRequest } from "./chat";

/** One run the scripted channel is holding. A test plays its events when it wants, so nothing waits on a clock. */
export interface ScriptedRun {
  readonly request: AgentRunRequest;
  /** True once the session asked to stop. */
  stopped: boolean;
  emit(event: ChannelEvent): void;
  event(event: AgentEvent): void;
}

export interface ScriptedAgentChannel extends AgentChannel {
  readonly runs: ScriptedRun[];
}

/** Stopping a scripted run says the abort and closes, as the real channel does. */
export const scriptedAgentChannel = (): ScriptedAgentChannel => {
  const runs: ScriptedRun[] = [];
  return {
    runs,
    run: (request, listener) => {
      let closed = false;
      const emit = (event: ChannelEvent) => {
        if (closed) return;
        listener(event);
        if (event.kind === "closed") closed = true;
      };
      const run: ScriptedRun = { request, stopped: false, emit, event: (event) => emit({ kind: "event", event }) };
      runs.push(run);
      return {
        stop: () => {
          run.stopped = true;
          emit({ kind: "event", event: { type: "run_error", message: "Request aborted", code: "abort" } });
          emit({ kind: "closed" });
        },
      };
    },
  };
};
