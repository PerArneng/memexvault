import { vaultId } from "@memex/types";
import { describe, expect, it } from "vitest";
import { parseWikiRoute, wikiPath } from "./wiki-routes";
import type { WikiRoute } from "./wiki-routes";

const q = (params: Readonly<Record<string, string>> = {}) => (name: string) => params[name] ?? null;
const v = vaultId("v 1");

describe("parseWikiRoute", () => {
  it("reads each screen", () => {
    expect(parseWikiRoute([], q())).toEqual({ kind: "list", view: "list", filter: "" });
    expect(parseWikiRoute([], q({ view: "tree", q: "a" }))).toEqual({ kind: "list", view: "tree", filter: "a" });
    expect(parseWikiRoute(["-", "new"], q({ title: "T" }))).toEqual({ kind: "new", title: "T" });
    expect(parseWikiRoute(["-", "broken-links"], q())).toEqual({ kind: "broken_links" });
    expect(parseWikiRoute(["-", "by-slug", "s"], q())).toEqual({ kind: "by_slug", slug: "s" });
    expect(parseWikiRoute(["p"], q())).toEqual({ kind: "read", id: "p", revision: null });
    expect(parseWikiRoute(["p"], q({ revision: "3" }))).toEqual({ kind: "read", id: "p", revision: 3 });
    expect(parseWikiRoute(["p", "edit"], q())).toEqual({ kind: "edit", id: "p" });
    expect(parseWikiRoute(["p", "history"], q())).toEqual({ kind: "history", id: "p" });
    expect(parseWikiRoute(["p", "diff"], q({ from: "1", to: "2" }))).toEqual({ kind: "diff", id: "p", from: 1, to: 2 });
  });

  it("lets a page have an id like a screen's name", () => {
    expect(parseWikiRoute(["new"], q())).toEqual({ kind: "read", id: "new", revision: null });
    expect(parseWikiRoute(["edit", "edit"], q())).toEqual({ kind: "edit", id: "edit" });
  });

  it("answers nonsense from a URL with `unknown`", () => {
    for (const rest of [["-"], ["-", "nope"], ["-", "new", "x"], ["-", "by-slug"], ["-", "by-slug", "a", "b"], ["p", "nope"], ["p", "edit", "x"], ["p", "diff"]]) {
      expect(parseWikiRoute(rest, q())).toEqual({ kind: "unknown" });
    }
    expect(parseWikiRoute(["p", "diff"], q({ from: "x", to: "2" }))).toEqual({ kind: "unknown" });
  });

  it("ignores a revision that isn't one", () => {
    for (const revision of ["x", "-1", "1.5", "", "99999999999"]) {
      expect(parseWikiRoute(["p"], q({ revision }))).toEqual({ kind: "read", id: "p", revision: null });
    }
  });
});

describe("wikiPath", () => {
  it("round-trips every screen, encoding what needs it", () => {
    const routes: Exclude<WikiRoute, { kind: "unknown" }>[] = [
      { kind: "list", view: "list", filter: "" },
      { kind: "list", view: "tree", filter: "a&b c" },
      { kind: "broken_links" },
      { kind: "new", title: "Å & co" },
      { kind: "by_slug", slug: "日本語/x" },
      { kind: "read", id: "p/1", revision: null },
      { kind: "read", id: "p", revision: 4 },
      { kind: "edit", id: "p" },
      { kind: "history", id: "p" },
      { kind: "diff", id: "p", from: 1, to: 2 },
    ];
    for (const route of routes) {
      const [path = "", search = ""] = wikiPath(v, route).split("?");
      const parts = path.split("/").filter((p) => p !== "").map(decodeURIComponent);
      const params = new Map(search.split("&").filter((p) => p !== "").map((p) => {
        const [name = "", value = ""] = p.split("=");
        return [name, decodeURIComponent(value)];
      }));
      expect(parts.slice(0, 3)).toEqual(["vaults", "v 1", "pages"]);
      expect(parseWikiRoute(parts.slice(3), (name) => params.get(name) ?? null)).toEqual(route);
    }
  });
});
