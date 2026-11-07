import { describe, expect, it } from "vitest";
import { INITIAL_SESSION, sessionReducer, signInErrorText } from "./session";
import type { SessionState } from "./session";
import { httpFailure, identityWith, UNREACHABLE } from "./testing";

const identity = identityWith([]);
const signedIn: SessionState = { status: "signed_in", identity };

describe("sessionReducer", () => {
  it("restores a session found at startup", () => {
    expect(sessionReducer(INITIAL_SESSION, { type: "restored", identity })).toEqual(signedIn);
  });
  it("shows sign-in with no complaint when there was no session", () => {
    expect(sessionReducer(INITIAL_SESSION, { type: "no_session" })).toEqual({ status: "signed_out", reason: "none" });
  });
  it("says expired when a live session gets a 401", () => {
    expect(sessionReducer(signedIn, { type: "unauthenticated" })).toEqual({ status: "signed_out", reason: "expired" });
  });
  it("says expired when a recheck finds a live session gone", () => {
    expect(sessionReducer(signedIn, { type: "no_session" })).toEqual({ status: "signed_out", reason: "expired" });
  });
  it("a 401 while signed out isn't an expiry", () => {
    const state: SessionState = { status: "signed_out", reason: "none" };
    expect(sessionReducer(state, { type: "unauthenticated" })).toBe(state);
  });
  it("a second 401 keeps the same state, so nothing renders again", () => {
    const expired = sessionReducer(signedIn, { type: "unauthenticated" });
    expect(sessionReducer(expired, { type: "unauthenticated" })).toBe(expired);
  });
  it("says signed out after signing out", () => {
    expect(sessionReducer(signedIn, { type: "signed_out" })).toEqual({ status: "signed_out", reason: "signed_out" });
  });
  it("a first check with no server leaves the session unknown, not signed out", () => {
    expect(sessionReducer(INITIAL_SESSION, { type: "check_failed" })).toEqual({ status: "unknown" });
  });
  it("a failed recheck doesn't sign a live session out", () => {
    expect(sessionReducer(signedIn, { type: "check_failed" })).toBe(signedIn);
    expect(sessionReducer(signedIn, { type: "checking" })).toBe(signedIn);
  });
});

describe("signInErrorText", () => {
  it("explains a wrong token", () => {
    expect(signInErrorText(httpFailure(401, "unauthenticated"))).toContain("wasn't accepted");
  });
  it("gives the wait after too many attempts", () => {
    expect(signInErrorText({ kind: "http", status: 429, code: "rate_limited", message: "x", requestId: "r", retryAfterS: 60 })).toContain("60 s");
    expect(signInErrorText(httpFailure(429, "rate_limited"))).toContain("a minute");
  });
  it("doesn't blame the token when the server is down", () => {
    expect(signInErrorText(UNREACHABLE)).toContain("Can't reach the server");
    expect(signInErrorText({ kind: "protocol", status: 502, message: "bad gateway" })).toContain("Can't reach the server");
  });
  it("names the request id on any other error", () => {
    expect(signInErrorText(httpFailure(500, "internal", "the request failed"))).toBe("the request failed (request id: req-1)");
  });
});
