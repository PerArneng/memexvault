import type { ReviewFilter, VaultId } from "@memex/types";
import { sectionPath } from "./navigation";

export const NO_FILTER: ReviewFilter = { extractor: "", promptVersion: "", itemType: "", proposer: "", verdict: "", sourceId: "" };

export const VERDICT_FILTERS: readonly { readonly id: string; readonly label: string }[] = [
  { id: "", label: "Any verdict" },
  { id: "approve", label: "Reviewer agent: approve" },
  { id: "reject", label: "Reviewer agent: reject" },
  { id: "abstain", label: "Reviewer agent: abstain" },
  { id: "none", label: "No verdict yet" },
];

/** Where a Review URL points. The `-` keeps a tab's name from ever meaning a ChangeSet, whatever id one has. */
export type ReviewRoute =
  | { readonly kind: "queue"; readonly filter: ReviewFilter; readonly item: string | null }
  | { readonly kind: "changesets"; readonly state: string }
  | { readonly kind: "changeset"; readonly id: string }
  | { readonly kind: "contradictions" }
  | { readonly kind: "auto_commit" }
  | { readonly kind: "unknown" };

const STATES: readonly string[] = ["pending", "committed", "rejected"];
const FILTER_PARAMS: readonly (readonly [keyof ReviewFilter, string])[] = [
  ["extractor", "extractor"],
  ["promptVersion", "prompt"],
  ["itemType", "type"],
  ["proposer", "proposer"],
  ["verdict", "verdict"],
  ["sourceId", "source"],
];

const filterFrom = (query: (name: string) => string | null): ReviewFilter => {
  const verdict = query("verdict") ?? "";
  return {
    extractor: query("extractor") ?? "",
    promptVersion: query("prompt") ?? "",
    itemType: query("type") ?? "",
    proposer: query("proposer") ?? "",
    // The server refuses a verdict it doesn't know, so a mistyped URL shows everything rather than an error.
    verdict: VERDICT_FILTERS.some((v) => v.id === verdict) ? verdict : "",
    sourceId: query("source") ?? "",
  };
};

/** `rest` is the path after `review`, already decoded. A URL anyone typed lands on `unknown` or a default rather than throwing. */
export const parseReviewRoute = (rest: readonly string[], query: (name: string) => string | null): ReviewRoute => {
  const [first, second, third] = rest;
  if (first === undefined) return { kind: "queue", filter: filterFrom(query), item: null };
  if (first !== "-") return second === undefined ? { kind: "queue", filter: filterFrom(query), item: first } : { kind: "unknown" };
  if (second === "changesets") {
    if (third !== undefined) return rest.length === 3 ? { kind: "changeset", id: third } : { kind: "unknown" };
    const state = query("state") ?? "";
    return { kind: "changesets", state: STATES.includes(state) ? state : "" };
  }
  if (third !== undefined) return { kind: "unknown" };
  if (second === "contradictions") return { kind: "contradictions" };
  if (second === "auto-commit") return { kind: "auto_commit" };
  return { kind: "unknown" };
};

export const reviewPath = (vault: VaultId, route: Exclude<ReviewRoute, { readonly kind: "unknown" }>): string => {
  const root = sectionPath(vault, "review");
  switch (route.kind) {
    case "queue": {
      const query = FILTER_PARAMS.filter(([field]) => route.filter[field] !== "").map(([field, name]) => `${name}=${encodeURIComponent(route.filter[field])}`).join("&");
      const path = route.item === null ? root : `${root}/${encodeURIComponent(route.item)}`;
      return query === "" ? path : `${path}?${query}`;
    }
    case "changesets":
      return route.state === "" ? `${root}/-/changesets` : `${root}/-/changesets?state=${encodeURIComponent(route.state)}`;
    case "changeset":
      return `${root}/-/changesets/${encodeURIComponent(route.id)}`;
    case "contradictions":
      return `${root}/-/contradictions`;
    case "auto_commit":
      return `${root}/-/auto-commit`;
  }
};
