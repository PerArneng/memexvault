import type { VaultId } from "@memex/types";
import type { EntityFilter, EntitySort, FilterOp } from "./entity-schema";
import { sectionPath } from "./navigation";

export interface EntityTableRoute {
  readonly kind: "table";
  readonly type: string;
  readonly filters: readonly EntityFilter[];
  /** Null means the columns the schema suggests. */
  readonly columns: readonly string[] | null;
  readonly sort: EntitySort | null;
  /** A mention being looked for with the resolve API. */
  readonly find: string;
}

/** Where an Entities URL points. The `-` keeps these words from ever meaning an entity, whatever id one has. */
export type EntityRoute =
  | { readonly kind: "types" }
  | { readonly kind: "new_type" }
  | EntityTableRoute
  | { readonly kind: "type"; readonly type: string }
  | { readonly kind: "edit_type"; readonly type: string }
  | { readonly kind: "new_entity"; readonly type: string }
  | { readonly kind: "same_as"; readonly type: string }
  | { readonly kind: "entity"; readonly id: string }
  | { readonly kind: "edit"; readonly id: string }
  | { readonly kind: "merge"; readonly id: string; readonly into: string }
  | { readonly kind: "split"; readonly id: string }
  | { readonly kind: "unknown" };

const OPS: readonly string[] = ["eq", "lt", "lte", "gt", "gte"];
const isOp = (raw: string): raw is FilterOp => OPS.includes(raw);

/** `<field>:<op>:<value>`, the API's own form. The value may hold colons. A piece anyone typed that can't be one is dropped, not thrown. */
export const parseEntityFilter = (raw: string): EntityFilter | null => {
  const first = raw.indexOf(":");
  const second = first < 0 ? -1 : raw.indexOf(":", first + 1);
  if (first <= 0 || second < 0) return null;
  const op = raw.slice(first + 1, second);
  return isOp(op) ? { field: raw.slice(0, first), op, value: raw.slice(second + 1) } : null;
};

export const entityFilterText = (filter: EntityFilter): string => `${filter.field}:${filter.op}:${filter.value}`;

/** All filters in one query value, a line each. The router rewrites a repeated parameter as a JSON array, and a filter's input is a single line. */
const FILTER_SEPARATOR = "\n";

const sortFrom = (raw: string | undefined): EntitySort | null => {
  if (raw === undefined) return null;
  const at = raw.lastIndexOf(":");
  if (at <= 0) return null;
  const direction = raw.slice(at + 1);
  return direction === "asc" || direction === "desc" ? { field: raw.slice(0, at), direction } : null;
};

/** `rest` is the path after `entities`, already decoded, and `query` gives every value of a name. A URL anyone typed lands on `unknown` rather than throwing. */
export const parseEntityRoute = (rest: readonly string[], query: (name: string) => readonly string[]): EntityRoute => {
  const [first, second, third, fourth, ...more] = rest;
  // The target of a merge is in the path, as the router reads a query value that looks like a number as one, and an id can.
  if (first === undefined) return { kind: "types" };
  if (more.length > 0) return { kind: "unknown" };
  if (first === "-") {
    if (second === "new-type" && third === undefined) return { kind: "new_type" };
    if (second !== "types" || third === undefined) return { kind: "unknown" };
    switch (fourth) {
      case undefined: {
        const columns = query("cols")[0];
        return {
          kind: "table",
          type: third,
          filters: (query("f")[0] ?? "").split(FILTER_SEPARATOR).flatMap((raw) => parseEntityFilter(raw) ?? []),
          columns: columns === undefined ? null : columns.split(",").filter((c) => c !== ""),
          sort: sortFrom(query("sort")[0]),
          find: query("find")[0] ?? "",
        };
      }
      case "schema":
        return { kind: "type", type: third };
      case "edit":
        return { kind: "edit_type", type: third };
      case "new":
        return { kind: "new_entity", type: third };
      case "same-as":
        return { kind: "same_as", type: third };
      default:
        return { kind: "unknown" };
    }
  }
  if (second === "merge" && fourth === undefined) return { kind: "merge", id: first, into: third ?? "" };
  if (third !== undefined || fourth !== undefined) return { kind: "unknown" };
  if (second === undefined) return { kind: "entity", id: first };
  if (second === "edit") return { kind: "edit", id: first };
  if (second === "split") return { kind: "split", id: first };
  return { kind: "unknown" };
};

const withQuery = (path: string, params: readonly (readonly [string, string])[]): string => {
  const query = params.map(([name, value]) => `${name}=${encodeURIComponent(value)}`).join("&");
  return query === "" ? path : `${path}?${query}`;
};

export const entityPath = (vault: VaultId, route: Exclude<EntityRoute, { readonly kind: "unknown" }>): string => {
  const root = sectionPath(vault, "entities");
  const typeRoot = (type: string) => `${root}/-/types/${encodeURIComponent(type)}`;
  const one = (id: string) => sectionPath(vault, "entities", id);
  switch (route.kind) {
    case "types":
      return root;
    case "new_type":
      return `${root}/-/new-type`;
    case "table":
      return withQuery(typeRoot(route.type), [
        ...(route.filters.length === 0 ? [] : [["f", route.filters.map(entityFilterText).join(FILTER_SEPARATOR)] as [string, string]]),
        ...(route.columns === null ? [] : [["cols", route.columns.join(",")] as [string, string]]),
        ...(route.sort === null ? [] : [["sort", `${route.sort.field}:${route.sort.direction}`] as [string, string]]),
        ...(route.find === "" ? [] : [["find", route.find] as [string, string]]),
      ]);
    case "type":
      return `${typeRoot(route.type)}/schema`;
    case "edit_type":
      return `${typeRoot(route.type)}/edit`;
    case "new_entity":
      return `${typeRoot(route.type)}/new`;
    case "same_as":
      return `${typeRoot(route.type)}/same-as`;
    case "entity":
      return one(route.id);
    case "edit":
      return `${one(route.id)}/edit`;
    case "merge":
      return route.into === "" ? `${one(route.id)}/merge` : `${one(route.id)}/merge/${encodeURIComponent(route.into)}`;
    case "split":
      return `${one(route.id)}/split`;
  }
};
