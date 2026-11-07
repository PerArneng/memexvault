import type { EpochMs } from "@memex/types";

/**
 * Spec 040 allows 5 s from a stopped server to the banner. A probe starts every 1.5 s and gives up after 1 s. Two failures in a row
 * mean offline, so one lost request doesn't flash the banner. The worst case is a server that hangs just after a good probe:
 * the next two probes start at 1.5 s and 3 s, and the second gives up at 4 s.
 */
export const PROBE_INTERVAL_MS = 1500;
export const PROBE_TIMEOUT_MS = 1000;
export const FAILURES_BEFORE_OFFLINE = 2;

export type ConnectivityState =
  | { readonly status: "online"; readonly failures: number }
  | { readonly status: "offline"; readonly since: EpochMs };

export type ConnectivityEvent = { readonly type: "reached" } | { readonly type: "failed"; readonly at: EpochMs };

export const INITIAL_CONNECTIVITY: ConnectivityState = { status: "online", failures: 0 };

/** Pure. Returns the same object when nothing changes, so views don't render again. */
export const connectivityReducer = (state: ConnectivityState, event: ConnectivityEvent): ConnectivityState => {
  if (event.type === "reached") {
    return state.status === "online" && state.failures === 0 ? state : INITIAL_CONNECTIVITY;
  }
  if (state.status === "offline") return state;
  const failures = state.failures + 1;
  return failures >= FAILURES_BEFORE_OFFLINE ? { status: "offline", since: event.at } : { status: "online", failures };
};
