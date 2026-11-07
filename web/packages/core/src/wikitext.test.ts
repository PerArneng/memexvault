import { INLINE_CLOSE, INLINE_OPEN } from "@memex/types";
import { describe, expect, it } from "vitest";
import { claimIdsIn, completionText, openReferenceAt, prepareWikiText, slugify } from "./wikitext";

const utf8Bytes = (text: string) => encodeURIComponent(text).replace(/%[0-9A-F]{2}/g, "x").length;
const links = (source: string) => prepareWikiText(source).tokens.filter((t) => t.kind === "link").map((t) => (t.kind === "link" ? t.slug : ""));

describe("slugify, on the server's own cases", () => {
  it("makes the same slugs", () => {
    expect(slugify("Hello,  World!")).toBe("hello-world");
    expect(slugify("Göteborg")).toBe("goteborg");
    expect(slugify("日本語 ページ")).toBe("日本語-ページ");
    expect(slugify("?!")).toBeNull();
    expect(slugify("--a--b--")).toBe("a-b");
    expect(slugify("Straße")).toBe("strasse");
  });

  it("stops at 200 bytes, on a whole character and not on a dash", () => {
    expect(slugify("å".repeat(500))?.length).toBe(200);
    const wide = slugify("語".repeat(100)) ?? "";
    expect(utf8Bytes(wide)).toBe(198);
    expect(slugify(`${"a".repeat(199)} b`)).toBe("a".repeat(199));
  });
});

describe("prepareWikiText", () => {
  it("finds links with and without labels, and claims", () => {
    const { text, tokens } = prepareWikiText("See [[Pump Room|the pump]] and {{claim:abc-1}} and [[other]].");
    expect(tokens).toEqual([
      { kind: "link", target: "Pump Room", slug: "pump-room", label: "the pump", raw: "[[Pump Room|the pump]]" },
      { kind: "claim", id: "abc-1", raw: "{{claim:abc-1}}" },
      { kind: "link", target: "other", slug: "other", label: null, raw: "[[other]]" },
    ]);
    expect(text).toBe(`See ${INLINE_OPEN}0${INLINE_CLOSE} and ${INLINE_OPEN}1${INLINE_CLOSE} and ${INLINE_OPEN}2${INLINE_CLOSE}.`);
  });

  it("leaves escapes, code and malformed references as text, as the server does", () => {
    expect(links("\\[[no]] `[[no]]` [[yes]]")).toEqual(["yes"]);
    expect(links("```\n[[no]]\n```\n\n[[yes]]")).toEqual(["yes"]);
    expect(links("~~~\n[[no]]\n\n[[still no]]\n~~~")).toEqual([]);
    expect(links("[[?!]] [[a\nb]] [[c[d]]")).toEqual([]);
    expect(prepareWikiText("{{claim:has space}} {{claim:}} \\{{claim:a}}").tokens).toEqual([]);
    expect(prepareWikiText(`{{claim:${"x".repeat(65)}}}`).tokens).toEqual([]);
  });

  it("lets a fence left open run to the end, and a blank line end a code span", () => {
    expect(links("```\n[[no]]\n\n[[no]]")).toEqual([]);
    // The backtick has no partner in its own block, so it's plain, and the link counts.
    expect(links("a ` b\n\n[[yes]] ` c")).toEqual(["yes"]);
  });

  it("keeps a page's stray placeholder marks from standing in for a reference", () => {
    const { text, tokens } = prepareWikiText(`${INLINE_OPEN}0${INLINE_CLOSE} [[a]]`);
    expect(tokens).toHaveLength(1);
    expect(text).toBe(`�0� ${INLINE_OPEN}0${INLINE_CLOSE}`);
  });

  it("treats CRLF as the newline it is", () => {
    expect(links("```\r\n[[no]]\r\n```\r\n\r\n[[yes]]")).toEqual(["yes"]);
  });

  it("names each claim once", () => {
    expect(claimIdsIn("{{claim:a}} {{claim:b}} {{claim:a}} `{{claim:c}}`")).toEqual(["a", "b"]);
  });
});

describe("openReferenceAt", () => {
  const at = (marked: string) => openReferenceAt(marked.replace("|^|", ""), marked.indexOf("|^|"));

  it("finds what's being typed", () => {
    expect(at("see [[pum|^|")).toEqual({ kind: "link", from: 6, query: "pum" });
    expect(at("see [[|^|")).toEqual({ kind: "link", from: 6, query: "" });
    expect(at("{{claim:pump op|^|")).toEqual({ kind: "claim", from: 8, query: "pump op" });
    expect(at("[[pum|^|]]")).toEqual({ kind: "link", from: 2, query: "pum" });
  });

  it("offers nothing outside a reference", () => {
    expect(at("plain|^|")).toBeNull();
    expect(at("[[done]] then|^|")).toBeNull();
    expect(at("{{claim:a}} then|^|")).toBeNull();
    expect(at("{{claim|^|")).toBeNull();
  });

  it("offers nothing in the label, in code, or after an escape", () => {
    expect(at("[[target|lab|^|")).toBeNull();
    expect(at("\\[[pum|^|")).toBeNull();
    expect(at("`[[pum|^|`")).toBeNull();
    expect(at("```\n[[pum|^|\n```")).toBeNull();
  });

  it("takes the last opener when two are open", () => {
    expect(at("[[a [[b|^|")).toEqual({ kind: "link", from: 6, query: "b" });
  });

  it("doesn't reach back across a blank line", () => {
    expect(at("[[a\n\nb|^|")).toBeNull();
    expect(at("[[a\nb|^|")).toBeNull();
  });
});

describe("completionText", () => {
  it("closes the reference unless it's already closed", () => {
    expect(completionText("link", "pump", " more")).toBe("pump]]");
    expect(completionText("link", "pump", "]] more")).toBe("pump");
    expect(completionText("claim", "c1", "")).toBe("c1}}");
    expect(completionText("claim", "c1", "}}")).toBe("c1");
  });
});
