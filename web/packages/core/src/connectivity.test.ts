import { epochMs } from "@memex/types";
import { describe, expect, it } from "vitest";
import { connectivityReducer, FAILURES_BEFORE_OFFLINE, INITIAL_CONNECTIVITY, PROBE_INTERVAL_MS, PROBE_TIMEOUT_MS } from "./connectivity";

const failAt = (at: number) => ({ type: "failed", at: epochMs(at) }) as const;

describe("connectivityReducer", () => {
  it("stays online while probes succeed, and keeps the same object", () => {
    expect(connectivityReducer(INITIAL_CONNECTIVITY, { type: "reached" })).toBe(INITIAL_CONNECTIVITY);
  });
  it("one lost request doesn't go offline", () => {
    expect(connectivityReducer(INITIAL_CONNECTIVITY, failAt(1)).status).toBe("online");
  });
  it("a success in between restarts the count", () => {
    const once = connectivityReducer(INITIAL_CONNECTIVITY, failAt(1));
    const again = connectivityReducer(connectivityReducer(once, { type: "reached" }), failAt(2));
    expect(again.status).toBe("online");
  });
  it("goes offline at the second failure in a row, and remembers when", () => {
    const state = connectivityReducer(connectivityReducer(INITIAL_CONNECTIVITY, failAt(1)), failAt(2));
    expect(state).toEqual({ status: "offline", since: 2 });
  });
  it("stays offline with the first time, and recovers at one success", () => {
    const offline = connectivityReducer(connectivityReducer(INITIAL_CONNECTIVITY, failAt(1)), failAt(2));
    expect(connectivityReducer(offline, failAt(3))).toBe(offline);
    expect(connectivityReducer(offline, { type: "reached" })).toEqual(INITIAL_CONNECTIVITY);
  });
  it("the worst case fits the 5 s the spec allows", () => {
    const lastProbeStart = PROBE_INTERVAL_MS * FAILURES_BEFORE_OFFLINE;
    expect(lastProbeStart + PROBE_TIMEOUT_MS).toBeLessThanOrEqual(5000);
    expect(PROBE_TIMEOUT_MS).toBeLessThan(PROBE_INTERVAL_MS);
  });
});
