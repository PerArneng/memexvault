import type { VaultId } from "@memex/types";
import { isZoomLevel } from "./overlay";
import { sectionPath } from "./navigation";

export type DocumentTab = "view" | "segments" | "claims" | "renditions" | "versions";
const TABS: readonly string[] = ["view", "segments", "claims", "renditions", "versions"];

export interface SourceFilter {
  readonly text: string;
  readonly tier: string;
  readonly kind: string;
  readonly flagged: boolean;
}

export interface SourceViewRoute {
  readonly kind: "source";
  readonly id: string;
  readonly tab: DocumentTab;
  /** 0-based here, and 1-based in the URL, as people read it. */
  readonly page: number | null;
  readonly zoom: number | null;
  /** Together they name a highlight. */
  readonly claim: string | null;
  readonly evidence: string | null;
  /** A time range of a recording, in milliseconds. */
  readonly fromMs: number | null;
  readonly toMs: number | null;
  /** A rendition other than the current one, for its segments. */
  readonly rendition: string | null;
  /** A passage to show, as a search citation names one. */
  readonly segment: string | null;
}

/** Where a Documents URL points. The `-` keeps `claims` from ever meaning a source, whatever id one has. */
export type DocumentRoute =
  | { readonly kind: "list"; readonly filter: SourceFilter }
  | SourceViewRoute
  | { readonly kind: "lineage"; readonly claim: string; readonly evidence: string | null }
  | { readonly kind: "unknown" };

const MAX_DIGITS = /^\d{1,12}$/;

const wholeNumber = (raw: string | null): number | null => (raw !== null && MAX_DIGITS.test(raw) ? Number(raw) : null);

const onePage = (raw: string | null): number | null => {
  const n = wholeNumber(raw);
  return n === null || n < 1 ? null : n - 1;
};

/** `rest` is the path after `documents`, already decoded. A URL anyone typed lands on `unknown` or a default rather than throwing. */
export const parseDocumentRoute = (rest: readonly string[], query: (name: string) => string | null): DocumentRoute => {
  const [first, second, third, ...more] = rest;
  if (first === undefined) {
    return { kind: "list", filter: { text: query("q") ?? "", tier: query("tier") ?? "", kind: query("kind") ?? "", flagged: query("flagged") === "1" } };
  }
  if (first === "-") {
    if (second === "claims" && third !== undefined && more.length === 0) return { kind: "lineage", claim: third, evidence: query("evidence") };
    return { kind: "unknown" };
  }
  if (second !== undefined) return { kind: "unknown" };
  const tab = query("tab");
  const zoom = wholeNumber(query("zoom"));
  const fromMs = wholeNumber(query("t"));
  const toMs = wholeNumber(query("end"));
  const claim = query("claim");
  const evidence = query("evidence");
  return {
    kind: "source",
    id: first,
    tab: tab !== null && TABS.includes(tab) ? (tab as DocumentTab) : "view",
    page: onePage(query("page")),
    zoom: zoom !== null && isZoomLevel(zoom) ? zoom : null,
    claim: claim !== null && evidence !== null ? claim : null,
    evidence: claim !== null && evidence !== null ? evidence : null,
    fromMs,
    // A range that ends before it starts has no end.
    toMs: fromMs !== null && toMs !== null && toMs > fromMs ? toMs : null,
    rendition: query("rendition"),
    segment: query("segment"),
  };
};

const withQuery = (path: string, params: readonly (readonly [string, string | null])[]): string => {
  const query = params.flatMap(([name, value]) => (value === null ? [] : [`${name}=${encodeURIComponent(value)}`])).join("&");
  return query === "" ? path : `${path}?${query}`;
};

const text = (n: number | null): string | null => (n === null ? null : String(n));
const notEmpty = (s: string): string | null => (s === "" ? null : s);

export const documentPath = (vault: VaultId, route: Exclude<DocumentRoute, { readonly kind: "unknown" }>): string => {
  const root = sectionPath(vault, "documents");
  switch (route.kind) {
    case "list":
      return withQuery(root, [["q", notEmpty(route.filter.text)], ["tier", notEmpty(route.filter.tier)], ["kind", notEmpty(route.filter.kind)], ["flagged", route.filter.flagged ? "1" : null]]);
    case "lineage":
      return withQuery(`${root}/-/claims/${encodeURIComponent(route.claim)}`, [["evidence", route.evidence]]);
    case "source":
      return withQuery(sectionPath(vault, "documents", route.id), [
        ["tab", route.tab === "view" ? null : route.tab],
        ["page", route.page === null ? null : String(route.page + 1)],
        ["zoom", text(route.zoom)],
        ["claim", route.claim],
        ["evidence", route.evidence],
        ["t", text(route.fromMs)],
        ["end", text(route.toMs)],
        ["rendition", route.rendition],
        ["segment", route.segment],
      ]);
  }
};
