import { epochMs } from "@memex/types";
import type { WikiPageSummary } from "@memex/types";
import { describe, expect, it } from "vitest";
import { filterPages, pageIndex, pageTree, rankPages } from "./wiki-pages";

const page = (slug: string, title = slug, updated = 0, redirects: readonly string[] = []): WikiPageSummary => ({ id: `id-${slug}`, title, slug, redirects, version: 1, creator: "pat", updatedAt: epochMs(updated) });

describe("rankPages", () => {
  const pages = [page("zebra", "Zebra", 3), page("pump-room", "Pump Room", 2), page("the-pump", "The Pump", 1), page("pumpkin", "Pumpkin", 4), page("sump", "Old sump", 5, ["pump-old"]), page("repump", "Repump", 6)];

  it("puts a slug or title start first, then a later word, then an old slug, then anywhere", () => {
    const got = rankPages(pages, "pump").map((r) => r.page.slug);
    expect(got).toEqual(["pump-room", "pumpkin", "the-pump", "sump", "repump"]);
  });

  it("says when an old slug is what matched", () => {
    expect(rankPages(pages, "pump-o")).toEqual([{ page: pages[4], viaRedirect: "pump-old" }]);
  });

  it("matches whatever people type: case, diacritics and spaces for dashes", () => {
    expect(rankPages([page("goteborg", "Göteborg")], "GÖTE").map((r) => r.page.slug)).toEqual(["goteborg"]);
    expect(rankPages([page("goteborg", "Göteborg")], "gote").map((r) => r.page.slug)).toEqual(["goteborg"]);
    expect(rankPages(pages, "pump r").map((r) => r.page.slug)).toEqual(["pump-room"]);
  });

  it("shows the newest changes when nothing is typed, and respects the limit", () => {
    expect(rankPages(pages, "", 2).map((r) => r.page.slug)).toEqual(["repump", "sump"]);
    expect(rankPages(pages, "  ", 1)).toHaveLength(1);
  });

  it("finds nothing rather than failing", () => {
    expect(rankPages(pages, "no such")).toEqual([]);
    expect(rankPages([], "x")).toEqual([]);
  });

  it("treats a typed bracket as text, not a pattern", () => {
    expect(rankPages([page("a", "a(b")], "a(").map((r) => r.page.title)).toEqual(["a(b"]);
  });
});

describe("pageIndex", () => {
  it("finds a page by a current or old slug, and nothing else", () => {
    const moved = page("new", "New", 0, ["old", "older"]);
    const index = pageIndex([moved]);
    expect(index.bySlug("new")).toBe(moved);
    expect(index.bySlug("older")).toBe(moved);
    expect(index.bySlug("other")).toBeNull();
    expect(index.bySlug("__proto__")).toBeNull();
  });
});

describe("pageTree", () => {
  const shape = (pages: readonly WikiPageSummary[]) => pageTree(pages).map((r) => `${" ".repeat(r.depth)}${r.page.slug}`);

  it("nests under the longest page whose slug is a dash-separated prefix", () => {
    expect(shape([page("alpha-notes-old"), page("beta"), page("alpha"), page("alpha-notes"), page("alpha-x-y")])).toEqual(["alpha", " alpha-notes", "  alpha-notes-old", " alpha-x-y", "beta"]);
  });

  it("needs a whole word: `alphabet` isn't under `alpha`", () => {
    expect(shape([page("alpha"), page("alphabet")])).toEqual(["alpha", "alphabet"]);
  });

  it("keeps every page, once, whatever the names", () => {
    const pages = [page("a"), page("a-a"), page("a-a-a"), page("b-c"), page("-")];
    expect(pageTree(pages).map((r) => r.page.slug).sort()).toEqual(["-", "a", "a-a", "a-a-a", "b-c"]);
    expect(pageTree([])).toEqual([]);
  });
});

describe("filterPages", () => {
  it("matches on title or slug, folded", () => {
    const pages = [page("goteborg", "Göteborg"), page("x-y", "Other")];
    expect(filterPages(pages, "GÖT").map((p) => p.slug)).toEqual(["goteborg"]);
    expect(filterPages(pages, "x-y").map((p) => p.slug)).toEqual(["x-y"]);
    expect(filterPages(pages, " ")).toBe(pages);
  });
});
