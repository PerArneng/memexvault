import type { Identity } from "@memex/types";
import { isUnauthenticated, requestIdText } from "./api";
import type { ApiFailure } from "./api";

export type SignedOutReason = "none" | "signed_out" | "expired";

export type SessionState =
  | { readonly status: "checking" }
  | { readonly status: "signed_out"; readonly reason: SignedOutReason }
  | { readonly status: "signed_in"; readonly identity: Identity }
  /** The server didn't answer the first check, so we can't tell yet. */
  | { readonly status: "unknown" };

export type SessionEvent =
  | { readonly type: "checking" }
  | { readonly type: "restored"; readonly identity: Identity }
  | { readonly type: "no_session" }
  | { readonly type: "check_failed" }
  | { readonly type: "signed_in"; readonly identity: Identity }
  | { readonly type: "signed_out" }
  /** Any call was refused with a 401. */
  | { readonly type: "unauthenticated" };

export const INITIAL_SESSION: SessionState = { status: "checking" };

/** Pure. Returns the same object when nothing changes, so views don't render again. */
export const sessionReducer = (state: SessionState, event: SessionEvent): SessionState => {
  switch (event.type) {
    case "checking":
      return state.status === "signed_in" ? state : { status: "checking" };
    case "restored":
    case "signed_in":
      return { status: "signed_in", identity: event.identity };
    case "no_session":
      // A check that finds nothing after we were signed in means the session ended underneath us.
      if (state.status === "signed_in") return { status: "signed_out", reason: "expired" };
      return state.status === "signed_out" ? state : { status: "signed_out", reason: "none" };
    case "check_failed":
      return state.status === "signed_in" ? state : { status: "unknown" };
    case "signed_out":
      return { status: "signed_out", reason: "signed_out" };
    case "unauthenticated":
      // Only a live session can expire. A wrong token at the sign-in form is not an expiry.
      return state.status === "signed_in" ? { status: "signed_out", reason: "expired" } : state;
  }
};

/** What the sign-in form says about a failure. It never repeats the token. */
export const signInErrorText = (failure: ApiFailure): string => {
  if (isUnauthenticated(failure)) return "That token wasn't accepted. Check that it's complete, not revoked and not expired.";
  if (failure.kind === "http" && failure.status === 429) {
    return failure.retryAfterS === undefined ? "Too many failed attempts. Try again in a minute." : `Too many failed attempts. Try again in ${String(failure.retryAfterS)} s.`;
  }
  if (failure.kind !== "http") return "Can't reach the server, so nothing was checked. Try again when it's back.";
  return `${failure.message} (${requestIdText(failure) ?? ""})`;
};
