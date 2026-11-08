import { vaultId } from "@memex/types";
import type { EvidenceItem, SearchCitation } from "@memex/types";
import { describe, expect, it } from "vitest";
import { changeSearch, citationPath, EMPTY_SEARCH, groupItems, itemPath, markQuote, markTerms, parseAsOf, parseInstant, parseSearchState, queryTerms, searchPath, searchProblem, searchRequestOf, toggled } from "./search";
import { httpFailure, UNREACHABLE } from "./testing";

const V = vaultId("v 1");
const parseUrl = (search: string) => {
  const params = new URLSearchParams(search);
  return parseSearchState((name) => params.get(name));
};
// The core's lib has no URLSearchParams, so the tests get their own.
declare class URLSearchParams {
  constructor(init: string);
  get(name: string): string | null;
}

const citation = (over: Partial<SearchCitation> = {}): SearchCitation => ({ anchor: "located", sourceId: "s1", sourceName: "a.pdf", sourceSha256: "x", trustTier: "trusted", segmentId: "seg1", evidenceId: null, renditionId: "r1", pageFrom: 2, pageTo: 3, rects: [], quote: null, tStartMs: null, tEndMs: null, speaker: null, ...over });
const item = (over: Partial<EvidenceItem> = {}): EvidenceItem => ({ id: "i1", kind: "segment", text: "t", heading: null, language: null, epistemic: null, trustTiers: [], validFromMs: null, validToMs: null, score: null, pageId: null, citations: [], conflicts: [], ...over });

describe("parseSearchState", () => {
  it("reads every parameter", () => {
    const { state, ignored } = parseUrl("?q=a%20b&kind=claim,segment&type=application/pdf&source=s1,s2&tier=trusted&epistemic=asserted&valid_at=2024-05-31&mode=lexical&expand=on&rerank=1&debug=1&min_seq=7&cursor=r20&limit=50");
    expect(ignored).toEqual([]);
    expect(state).toEqual({ q: "a b", kinds: ["claim", "segment"], types: ["application/pdf"], sources: ["s1", "s2"], tiers: ["trusted"], epistemic: ["asserted"], validAt: "2024-05-31", asOf: "", mode: "lexical", expand: "on", rerank: true, debug: true, minSeq: 7, cursor: "r20", limit: 50 });
  });

  it("gives the defaults for an empty URL", () => {
    expect(parseUrl("")).toEqual({ state: EMPTY_SEARCH, ignored: [] });
  });

  it.each([
    ["?kind=claim,nonsense", "kind", { kinds: ["claim"] }],
    ["?tier=gold", "tier", { tiers: [] }],
    ["?epistemic=maybe", "epistemic", { epistemic: [] }],
    ["?mode=fast", "mode", { mode: "hybrid" }],
    ["?expand=yes", "expand", { expand: "auto" }],
    ["?rerank=true", "rerank", { rerank: false }],
    ["?debug=2", "debug", { debug: false }],
    ["?min_seq=-1", "min_seq", { minSeq: null }],
    ["?min_seq=abc", "min_seq", { minSeq: null }],
    ["?min_seq=99999999999999999999", "min_seq", { minSeq: null }],
    ["?limit=0", "limit", { limit: 20 }],
    ["?limit=101", "limit", { limit: 20 }],
    ["?limit=1e2", "limit", { limit: 20 }],
    ["?valid_at=yesterday", "valid_at", { validAt: "" }],
    ["?valid_at=2024-02-30", "valid_at", { validAt: "" }],
    ["?as_of=soon", "as_of", { asOf: "" }],
    ["?as_of=-5", "as_of", { asOf: "" }],
    ["?q=a%00b", "q", { q: "" }],
    [`?q=${"x".repeat(2001)}`, "q", { q: "" }],
  ] as const)("names what it can't read in %s, and goes on without it", (url, name, expected) => {
    const { state, ignored } = parseUrl(url);
    expect(ignored).toEqual([name]);
    expect(state).toMatchObject(expected);
  });

  it("survives URLs that aren't well formed", () => {
    for (const url of ["?q=%", "?q=%E0%A4%A", "?%%%=%%%", "?q", "?=", "?kind=,,,", "?q=a&q=b", "?cursor=", "&&&"]) expect(() => parseUrl(url)).not.toThrow();
    expect(parseUrl("?q=%").state.q).toBe("%");
    expect(parseUrl("?kind=,,,").state.kinds).toEqual([]);
    expect(parseUrl("?q=a&q=b").state.q).toBe("a");
    expect(parseUrl("?cursor=").state.cursor).toBeNull();
  });

  it("limits a free list rather than sending thousands of values", () => {
    const { state, ignored } = parseUrl(`?source=${Array.from({ length: 30 }, (_, i) => `s${String(i)}`).join(",")}`);
    expect(state.sources).toHaveLength(20);
    expect(ignored).toEqual(["source"]);
  });

  it("removes repeated values", () => {
    expect(parseUrl("?source=a,b,a,%20b").state.sources).toEqual(["a", "b"]);
  });

  it("leaves out what a past state can't do, and says so", () => {
    const { state, ignored } = parseUrl("?as_of=12&rerank=1&min_seq=3&mode=vector");
    expect(state).toMatchObject({ asOf: "12", rerank: false, minSeq: null, mode: "hybrid" });
    expect(ignored).toEqual(["rerank", "min_seq", "mode"]);
  });

  it("keeps a query's text as it is, whatever it holds", () => {
    for (const q of ["<img src=x onerror=alert(1)>", "räksmörgås Straße niño", "\"a phrase\" AND b", "doc-2024/17#a&b=c", "  spaces  ", "100%"]) {
      const built = searchPath(V, { ...EMPTY_SEARCH, q });
      expect(parseUrl(built.slice(built.indexOf("?"))).state.q).toBe(q);
    }
  });
});

describe("searchPath", () => {
  it("is the bare screen for the defaults", () => {
    expect(searchPath(V, EMPTY_SEARCH)).toBe("/vaults/v%201/search");
  });

  it("reads back as the same state", () => {
    const state = parseUrl("?q=a&kind=claim&type=t&source=s&tier=verified&epistemic=inferred&valid_at=2024-05-31T14:30&expand=off&mode=vector&rerank=1&debug=1&min_seq=9&cursor=r5&limit=30").state;
    const built = searchPath(V, state);
    expect(parseUrl(built.slice(built.indexOf("?")))).toEqual({ state, ignored: [] });
  });

  it("escapes what would break out of its parameter", () => {
    expect(searchPath(V, { ...EMPTY_SEARCH, q: "a&b=c#d" })).toBe("/vaults/v%201/search?q=a%26b%3Dc%23d");
  });
});

describe("changeSearch", () => {
  it("goes back to the first page, unless the change names a page", () => {
    const paged = { ...EMPTY_SEARCH, q: "a", cursor: "r20" };
    expect(changeSearch(paged, { q: "b" }).cursor).toBeNull();
    expect(changeSearch(paged, { cursor: "r40" }).cursor).toBe("r40");
  });

  it("turns off what a past state can't do", () => {
    expect(changeSearch({ ...EMPTY_SEARCH, rerank: true, minSeq: 3, mode: "vector" }, { asOf: "5" })).toMatchObject({ rerank: false, minSeq: null, mode: "hybrid", asOf: "5" });
  });

  it("toggles a value in a list", () => {
    expect(toggled(["a"], "b")).toEqual(["a", "b"]);
    expect(toggled(["a", "b"], "a")).toEqual(["b"]);
  });
});

describe("searchRequestOf", () => {
  it("sends nothing for an empty query, or one of only spaces", () => {
    expect(searchRequestOf(EMPTY_SEARCH)).toBeNull();
    expect(searchRequestOf({ ...EMPTY_SEARCH, q: " \t " })).toBeNull();
  });

  it("sends an identifier as it was typed, so only the API decides what it is", () => {
    expect(searchRequestOf({ ...EMPTY_SEARCH, q: "  DOC-2024/17 " })?.q).toBe("  DOC-2024/17 ");
  });

  it("turns as_of into an event or a time, and valid_at into a time", () => {
    expect(searchRequestOf({ ...EMPTY_SEARCH, q: "a", asOf: "12" })).toMatchObject({ asOfEvent: 12, asOfMs: null });
    expect(searchRequestOf({ ...EMPTY_SEARCH, q: "a", asOf: "2024-01-01" })).toMatchObject({ asOfEvent: null, asOfMs: 1704067200000 });
    expect(searchRequestOf({ ...EMPTY_SEARCH, q: "a", validAt: "2024-01-01T00:00:01Z" })).toMatchObject({ validAtMs: 1704067201000 });
  });

  it("gives each page room for its items", () => {
    expect(searchRequestOf({ ...EMPTY_SEARCH, q: "a", limit: 50 })).toMatchObject({ limit: 50, budgetTokens: 20000 });
  });
});

describe("parseInstant and parseAsOf", () => {
  it("reads dates and times, in UTC", () => {
    expect(parseInstant("1970-01-02")).toBe(86400000);
    expect(parseInstant("1970-01-01T00:01")).toBe(60000);
    expect(parseInstant("1970-01-01 00:00:05")).toBe(5000);
    expect(parseInstant("2024-02-29")).not.toBeNull();
  });

  it.each(["", "2023-02-29", "2024-13-01", "2024-00-10", "2024-01-32", "2024-01-01T24:00", "2024-01-01T10:60", "01/02/2024", "2024-1-1", "now"])("refuses %s", (raw) => {
    expect(parseInstant(raw)).toBeNull();
  });

  it("takes a plain number as an event", () => {
    expect(parseAsOf("0")).toEqual({ kind: "event", seq: 0 });
    expect(parseAsOf(" 12 ")).toEqual({ kind: "event", seq: 12 });
    expect(parseAsOf("1970-01-01")).toEqual({ kind: "time", ms: 0 });
    expect(parseAsOf("x")).toBeNull();
  });
});

const joined = (parts: readonly { readonly text: string }[]) => parts.map((p) => p.text).join("");
const marked = (parts: readonly { readonly text: string; readonly marked: boolean }[]) => parts.filter((p) => p.marked).map((p) => p.text);

describe("queryTerms", () => {
  it("takes words and quoted phrases, longest first, and leaves out the short and the repeated", () => {
    expect(queryTerms('"red fox" jumps a jumps')).toEqual(["red fox", "jumps"]);
  });

  it("keeps a short word when that's all there is", () => {
    expect(queryTerms("ab")).toEqual(["ab"]);
  });

  it("strips punctuation at a word's ends, and keeps letters of any language", () => {
    expect(queryTerms("(Straße), år! niño?")).toEqual(["Straße", "niño"]);
    expect(queryTerms("smörgås")).toEqual(["smörgås"]);
  });

  it("copes with an unclosed quote and with nothing", () => {
    expect(queryTerms('"unclosed')).toEqual(["unclosed"]);
    expect(queryTerms("  ")).toEqual([]);
    expect(queryTerms('""')).toEqual([]);
  });
});

describe("markTerms", () => {
  it.each([
    ["The Straße in Köln", ["straße"], ["Straße"]],
    ["RÄKSMÖRGÅS och smörgås", ["räksmörgås", "smörgås"], ["RÄKSMÖRGÅS", "smörgås"]],
    ["El niño y la NIÑA", ["niño"], ["niño"]],
    ["ÜBER über", ["über"], ["ÜBER", "über"]],
  ])("finds words in any case, keeping their text: %s", (text, terms, found) => {
    const parts = markTerms(text, terms);
    expect(joined(parts)).toBe(text);
    expect(marked(parts)).toEqual(found);
  });

  it("marks where a word starts, not inside another", () => {
    expect(marked(markTerms("the cat concat cats", ["cat"]))).toEqual(["cat", "cat"]);
  });

  it("takes terms literally, whatever they hold", () => {
    const text = "a.b axb (c) [d] e+f g*h <img>";
    expect(marked(markTerms(text, ["a.b", "(c", "[d]", "e+f", "g*h", "<img>"]))).toEqual(["a.b", "(c", "[d]", "e+f", "g*h", "<img>"]);
    expect(joined(markTerms(text, ["a.b"]))).toBe(text);
  });

  it("gives the text whole when there's nothing to mark", () => {
    expect(markTerms("abc", [])).toEqual([{ text: "abc", marked: false }]);
    expect(markTerms("", [])).toEqual([]);
    expect(markTerms("abc", ["zzz"])).toEqual([{ text: "abc", marked: false }]);
  });
});

describe("markQuote", () => {
  it("marks a quote, apart from runs of whitespace, and keeps the text exact", () => {
    const text = "Vor der Straße\n  stand ein Baum. Danach";
    const parts = markQuote(text, "der Straße stand ein");
    expect(joined(parts)).toBe(text);
    expect(marked(parts)).toEqual(["der Straße\n  stand ein"]);
  });

  it("marks nothing when the quote isn't there, or is empty", () => {
    expect(marked(markQuote("abc", "xyz"))).toEqual([]);
    expect(marked(markQuote("abc", "  "))).toEqual([]);
    expect(joined(markQuote("abc", "xyz"))).toBe("abc");
  });

  it("takes a quote literally", () => {
    expect(marked(markQuote("cost (in $) is", "(in $)"))).toEqual(["(in $)"]);
  });
});

describe("groupItems", () => {
  it("puts claims first, keeps the API's order within a kind, and shows a kind it doesn't know last", () => {
    const groups = groupItems([item({ id: "1" }), item({ id: "2", kind: "claim" }), item({ id: "3" }), item({ id: "4", kind: "section" }), item({ id: "5", kind: "wormhole" })]);
    expect(groups.map((g) => g.kind)).toEqual(["claim", "segment", "section", "wormhole"]);
    expect(groups[1]?.items.map((i) => i.id)).toEqual(["1", "3"]);
  });

  it("has no groups for nothing", () => {
    expect(groupItems([])).toEqual([]);
  });
});

describe("citationPath", () => {
  it("opens a segment's page and names the passage", () => {
    expect(citationPath(V, item(), citation())).toBe("/vaults/v%201/documents/s1?page=3&segment=seg1");
  });

  it("opens a claim's evidence at its first rectangle", () => {
    const rects = [{ page: 4, x0: 0, y0: 0, x1: 1, y1: 1 }];
    expect(citationPath(V, item({ id: "c1", kind: "claim" }), citation({ evidenceId: "e1", rects }))).toBe("/vaults/v%201/documents/s1?page=5&claim=c1&evidence=e1");
  });

  it("opens a recording at its time", () => {
    expect(citationPath(V, item(), citation({ pageFrom: null, pageTo: null, tStartMs: 1000, tEndMs: 2000 }))).toBe("/vaults/v%201/documents/s1?t=1000&end=2000&segment=seg1");
  });

  it("doesn't call an entity's evidence a claim of that entity's id", () => {
    expect(citationPath(V, item({ id: "ent", kind: "entity" }), citation({ evidenceId: "e1" }))).toBe("/vaults/v%201/documents/s1?page=3&segment=seg1");
  });

  it("has nowhere to go without a source", () => {
    expect(citationPath(V, item(), citation({ sourceId: null }))).toBeNull();
  });

  it("escapes its ids", () => {
    expect(citationPath(V, item(), citation({ sourceId: "a/b", segmentId: "c&d", pageFrom: null }))).toBe("/vaults/v%201/documents/a%2Fb?segment=c%26d");
  });
});

describe("itemPath", () => {
  it("knows where each kind lives", () => {
    expect(itemPath(V, { id: "b1", kind: "block", pageId: "p1" })).toBe("/vaults/v%201/pages/p1");
    expect(itemPath(V, { id: "e1", kind: "entity", pageId: null })).toBe("/vaults/v%201/entities/e1");
    expect(itemPath(V, { id: "c1", kind: "claim", pageId: null })).toBe("/vaults/v%201/documents/-/claims/c1");
    expect(itemPath(V, { id: "s1", kind: "segment", pageId: null })).toBeNull();
    expect(itemPath(V, { id: "p", kind: "page", pageId: null })).toBeNull();
  });
});

describe("searchProblem", () => {
  const paged = { ...EMPTY_SEARCH, q: "a", cursor: "bogus" };
  const waiting = { ...EMPTY_SEARCH, q: "a", minSeq: 999 };

  it("tells the failures a person can act on from the rest", () => {
    expect(searchProblem(httpFailure(400, "invalid", "that cursor isn't one this API gave out"), paged)).toEqual({ kind: "bad_cursor" });
    expect(searchProblem(httpFailure(400, "invalid", "min_seq 999 is ahead of the log, whose last event is Some(4)"), waiting)).toEqual({ kind: "min_seq_ahead", minSeq: 999 });
    expect(searchProblem(httpFailure(503, "not_caught_up"), waiting)).toEqual({ kind: "not_caught_up" });
    expect(searchProblem(httpFailure(429, "busy"), EMPTY_SEARCH)).toEqual({ kind: "busy" });
  });

  it("doesn't claim a cause the state can't have", () => {
    expect(searchProblem(httpFailure(400, "invalid", "that cursor isn't one this API gave out"), EMPTY_SEARCH)).toEqual({ kind: "other" });
    expect(searchProblem(httpFailure(400, "invalid", "min_seq is ahead"), EMPTY_SEARCH)).toEqual({ kind: "other" });
    expect(searchProblem(httpFailure(400, "invalid", "something else"), paged)).toEqual({ kind: "other" });
    expect(searchProblem(httpFailure(503, "unavailable"), EMPTY_SEARCH)).toEqual({ kind: "other" });
    expect(searchProblem(UNREACHABLE, EMPTY_SEARCH)).toEqual({ kind: "other" });
  });
});
