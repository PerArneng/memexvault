import { isForbidden, isNotFound, isUnauthenticated, requestIdText } from "./api";
import type { ApiFailure } from "./api";

export interface Notice {
  readonly id: string;
  readonly title: string;
  readonly detail?: string;
}

/** A read shows 403 and 404 on the screen itself. An action has no such place, so it gets a toast. */
export type FailureSource = "read" | "action";

/**
 * What a failure says as a toast, or null if something else already says it: the offline banner for no answer, and the
 * sign-in page for an expired session.
 */
export const noticeFor = (failure: ApiFailure, source: FailureSource): Omit<Notice, "id"> | null => {
  if (failure.kind === "unreachable" || isUnauthenticated(failure)) return null;
  if (source === "read" && (isForbidden(failure) || isNotFound(failure))) return null;
  const id = requestIdText(failure);
  return id === null ? { title: failure.message } : { title: failure.message, detail: id };
};
