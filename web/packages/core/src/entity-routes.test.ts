import { vaultId } from "@memex/types";
import { describe, expect, it } from "vitest";
import { entityPath, parseEntityFilter, parseEntityRoute } from "./entity-routes";
import type { EntityRoute } from "./entity-routes";

const vault = vaultId("v1");

const parsePath = (path: string): EntityRoute => {
  const [pathname = "", search = ""] = path.split("?");
  const rest = pathname.split("/").filter((p) => p !== "").slice(3).map(decodeURIComponent);
  const pairs = search.split("&").filter((q) => q !== "").map((q) => q.split("=").map(decodeURIComponent));
  return parseEntityRoute(rest, (name) => pairs.filter(([n]) => n === name).map(([, value]) => value ?? ""));
};

describe("entity routes", () => {
  const routes: readonly Exclude<EntityRoute, { readonly kind: "unknown" }>[] = [
    { kind: "types" },
    { kind: "new_type" },
    { kind: "table", type: "Product", filters: [], columns: null, sort: null, find: "" },
    { kind: "table", type: "A b/c", filters: [{ field: "price", op: "lt", value: "100" }, { field: "at", op: "gte", value: "2026-01-01T10:00:00Z" }, { field: "name", op: "eq", value: "x&y=z" }], columns: ["a", "b"], sort: { field: "price", direction: "desc" }, find: "rät sum" },
    { kind: "type", type: "Product" },
    { kind: "edit_type", type: "Product" },
    { kind: "new_entity", type: "Product" },
    { kind: "same_as", type: "Product" },
    { kind: "entity", id: "e/1" },
    { kind: "edit", id: "e1" },
    { kind: "merge", id: "e1", into: "" },
    { kind: "merge", id: "e1", into: "e2" },
    { kind: "split", id: "e1" },
  ];
  for (const route of routes) {
    it(`round-trips ${route.kind} ${JSON.stringify(route).length}`, () => {
      expect(parsePath(entityPath(vault, route))).toEqual(route);
    });
  }

  it("lands on unknown rather than throwing, for anything typed", () => {
    for (const rest of [["-"], ["-", "nope"], ["-", "types"], ["-", "types", "T", "nope"], ["e1", "nope"], ["e1", "merge", "e2", "more"], ["e1", "split", "more"], ["e1", "edit", "more"], ["-", "types", "T", "new", "more"]]) {
      expect(parseEntityRoute(rest, () => []).kind).toBe("unknown");
    }
  });

  it("drops filters and sorts that can't be one", () => {
    const query = (name: string) => ({ f: ["nope\na:bad:1\n:eq:1\na:eq:1"], sort: ["x"] })[name] ?? [];
    const route = parseEntityRoute(["-", "types", "T"], query);
    expect(route).toMatchObject({ filters: [{ field: "a", op: "eq", value: "1" }], sort: null });
  });

  it("keeps colons in a value", () => {
    expect(parseEntityFilter("at:gte:2026-01-01T10:00:00Z")).toEqual({ field: "at", op: "gte", value: "2026-01-01T10:00:00Z" });
    expect(parseEntityFilter("name:eq:")).toEqual({ field: "name", op: "eq", value: "" });
  });
});
