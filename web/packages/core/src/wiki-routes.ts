import type { VaultId } from "@memex/types";
import { sectionPath } from "./navigation";

/** Where a Pages URL points. The `-` keeps `new` and `by-slug` from ever meaning a page, whatever id one has. */
export type WikiRoute =
  | { readonly kind: "list"; readonly view: "list" | "tree"; readonly filter: string }
  | { readonly kind: "broken_links" }
  | { readonly kind: "new"; readonly title: string }
  | { readonly kind: "by_slug"; readonly slug: string }
  | { readonly kind: "read"; readonly id: string; readonly revision: number | null }
  | { readonly kind: "edit"; readonly id: string }
  | { readonly kind: "history"; readonly id: string }
  | { readonly kind: "diff"; readonly id: string; readonly from: number; readonly to: number }
  | { readonly kind: "unknown" };

const revisionIn = (raw: string | null): number | null => {
  if (raw === null || !/^\d{1,9}$/.test(raw)) return null;
  return Number(raw);
};

/** `rest` is the path after `pages`, already decoded, and `query` reads its query string. A URL anyone typed lands on `unknown` rather than throwing. */
export const parseWikiRoute = (rest: readonly string[], query: (name: string) => string | null): WikiRoute => {
  const [first, second, third, ...more] = rest;
  if (first === undefined) return { kind: "list", view: query("view") === "tree" ? "tree" : "list", filter: query("q") ?? "" };
  if (first === "-") {
    if (second === "new" && third === undefined) return { kind: "new", title: query("title") ?? "" };
    if (second === "broken-links" && third === undefined) return { kind: "broken_links" };
    if (second === "by-slug" && third !== undefined && more.length === 0) return { kind: "by_slug", slug: third };
    return { kind: "unknown" };
  }
  if (third !== undefined) return { kind: "unknown" };
  if (second === undefined) return { kind: "read", id: first, revision: revisionIn(query("revision")) };
  if (second === "edit") return { kind: "edit", id: first };
  if (second === "history") return { kind: "history", id: first };
  if (second === "diff") {
    const from = revisionIn(query("from"));
    const to = revisionIn(query("to"));
    return from === null || to === null ? { kind: "unknown" } : { kind: "diff", id: first, from, to };
  }
  return { kind: "unknown" };
};

const withQuery = (path: string, params: Readonly<Record<string, string>>): string => {
  const query = Object.entries(params).map(([name, value]) => `${name}=${encodeURIComponent(value)}`).join("&");
  return query === "" ? path : `${path}?${query}`;
};

const pagePath = (vault: VaultId, id: string, tail: string): string => `${sectionPath(vault, "pages", id)}${tail}`;

export const wikiPath = (vault: VaultId, route: Exclude<WikiRoute, { readonly kind: "unknown" }>): string => {
  const root = sectionPath(vault, "pages");
  switch (route.kind) {
    case "list":
      return withQuery(root, { ...(route.view === "tree" ? { view: "tree" } : {}), ...(route.filter === "" ? {} : { q: route.filter }) });
    case "broken_links":
      return `${root}/-/broken-links`;
    case "new":
      return withQuery(`${root}/-/new`, route.title === "" ? {} : { title: route.title });
    case "by_slug":
      return `${root}/-/by-slug/${encodeURIComponent(route.slug)}`;
    case "read":
      return withQuery(pagePath(vault, route.id, ""), route.revision === null ? {} : { revision: String(route.revision) });
    case "edit":
      return pagePath(vault, route.id, "/edit");
    case "history":
      return pagePath(vault, route.id, "/history");
    case "diff":
      return withQuery(pagePath(vault, route.id, "/diff"), { from: String(route.from), to: String(route.to) });
  }
};
