import { vaultId } from "@memex/types";
import { describe, expect, it } from "vitest";
import { documentPath, parseDocumentRoute } from "./document-routes";
import type { DocumentRoute } from "./document-routes";

const V = vaultId("v1");
const parse = (path: string): DocumentRoute => {
  const [rest = "", search = ""] = path.split("?");
  const query = new URLSearchParams(search);
  return parseDocumentRoute(rest.split("/").filter((p) => p !== "").map(decodeURIComponent), (name) => query.get(name));
};
const source = (path: string) => {
  const route = parse(path);
  if (route.kind !== "source") throw new Error(`expected a source, got ${route.kind}`);
  return route;
};

describe("a Documents URL", () => {
  it("with nothing more is the list, with its filters", () => {
    expect(parse("?q=report&tier=trusted&kind=pdf&flagged=1")).toEqual({ kind: "list", filter: { text: "report", tier: "trusted", kind: "pdf", flagged: true } });
    expect(parse("")).toEqual({ kind: "list", filter: { text: "", tier: "", kind: "", flagged: false } });
  });

  it("names a source, a page, a zoom and a highlight", () => {
    const route = source("s1?page=3&zoom=150&claim=a1&evidence=e1");
    expect(route).toMatchObject({ id: "s1", page: 2, zoom: 150, claim: "a1", evidence: "e1", tab: "view" });
  });

  it("names a time range of a recording", () => {
    expect(source("s1?t=61000&end=64000")).toMatchObject({ fromMs: 61000, toMs: 64000 });
  });

  it("has a claim's lineage under `-`, so no source's id can mean it", () => {
    expect(parse("-/claims/a1?evidence=e1")).toEqual({ kind: "lineage", claim: "a1", evidence: "e1" });
    expect(parse("-/claims/a1")).toEqual({ kind: "lineage", claim: "a1", evidence: null });
  });

  it.each(["-", "-/nope", "-/claims", "-/claims/a/b", "s1/extra"])("doesn't throw on %s, and points nowhere", (path) => {
    expect(parse(path)).toEqual({ kind: "unknown" });
  });

  it.each(["page=0", "page=-3", "page=abc", "page=1e9", "page=99999999999999"])("ignores %s", (q) => {
    expect(source(`s1?${q}`).page).toBeNull();
  });

  it("ignores a zoom that isn't a level, and a tab that isn't one", () => {
    expect(source("s1?zoom=137").zoom).toBeNull();
    expect(source("s1?tab=<script>").tab).toBe("view");
  });

  it("needs both halves of a highlight", () => {
    expect(source("s1?claim=a1")).toMatchObject({ claim: null, evidence: null });
    expect(source("s1?evidence=e1")).toMatchObject({ claim: null, evidence: null });
  });

  it("drops the end of a range that ends before it starts, and an end with no start", () => {
    expect(source("s1?t=5000&end=1000")).toMatchObject({ fromMs: 5000, toMs: null });
    expect(source("s1?end=1000")).toMatchObject({ fromMs: null, toMs: null });
  });
});

describe("a path", () => {
  it.each([
    "s1",
    "s1?page=3&zoom=150",
    "s1?tab=segments&rendition=r1",
    "s1?page=1&claim=a1&evidence=e1",
    "s1?t=61000&end=64000",
    "-/claims/a1?evidence=e1",
    "?q=a%20b&tier=trusted&flagged=1",
  ])("for %s reads back as the same route", (path) => {
    const route = parse(path);
    if (route.kind === "unknown") throw new Error("unexpected");
    const built = documentPath(V, route);
    expect(built.startsWith("/vaults/v1/documents")).toBe(true);
    expect(parse(built.replace("/vaults/v1/documents", "").replace(/^\//, ""))).toEqual(route);
  });

  it("encodes what it's given", () => {
    const built = documentPath(V, { kind: "lineage", claim: "a/b?c", evidence: "e&f" });
    expect(built).toBe("/vaults/v1/documents/-/claims/a%2Fb%3Fc?evidence=e%26f");
  });

  it("has no query at all for a bare source, so opening one leaves its address alone", () => {
    expect(documentPath(V, source("s1"))).toBe("/vaults/v1/documents/s1");
  });
});
