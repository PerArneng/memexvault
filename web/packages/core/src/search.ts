import type { ApiFailure, EvidenceItem, SearchCitation, VaultId } from "@memex/types";
import type { SourceViewRoute } from "./document-routes";
import { documentPath } from "./document-routes";
import { TRUST_TIERS } from "./documents";
import { sectionPath } from "./navigation";

export const SEARCH_KINDS = ["claim", "segment", "entity", "page", "block"] as const;
export const SEARCH_EPISTEMIC = ["verified", "asserted", "inferred", "disputed", "superseded"] as const;
export const SEARCH_MODES = ["hybrid", "lexical", "vector"] as const;
export const SEARCH_EXPANDS = ["auto", "on", "off"] as const;
export type SearchMode = (typeof SEARCH_MODES)[number];
export type SearchExpand = (typeof SEARCH_EXPANDS)[number];

/** How long typing has to pause before a search runs. */
export const SEARCH_DEBOUNCE_MS = 250;
export const SEARCH_DEFAULT_LIMIT = 20;
/** A person can read more than an agent's default budget, and a text is never cut, so the budget follows the page size at about a long segment each. */
const TOKENS_PER_ITEM = 400;
const MAX_QUERY_CHARS = 2000;
const MAX_VALUES = 20;

/** Everything a search screen shows comes from this, and all of it is in the URL. */
export interface SearchState {
  readonly q: string;
  readonly kinds: readonly string[];
  readonly types: readonly string[];
  readonly sources: readonly string[];
  readonly tiers: readonly string[];
  readonly epistemic: readonly string[];
  /** As it's written in the URL, and always readable by `parseInstant`. Empty when unset. */
  readonly validAt: string;
  /** As it's written in the URL, and always readable by `parseAsOf`. Empty when unset. */
  readonly asOf: string;
  readonly mode: SearchMode;
  readonly expand: SearchExpand;
  readonly rerank: boolean;
  readonly debug: boolean;
  readonly minSeq: number | null;
  readonly cursor: string | null;
  readonly limit: number;
}

export const EMPTY_SEARCH: SearchState = { q: "", kinds: [], types: [], sources: [], tiers: [], epistemic: [], validAt: "", asOf: "", mode: "hybrid", expand: "auto", rerank: false, debug: false, minSeq: null, cursor: null, limit: SEARCH_DEFAULT_LIMIT };

/** What the API's search route takes. */
export interface SearchRequest {
  readonly q: string;
  readonly kinds: readonly string[];
  readonly types: readonly string[];
  readonly sources: readonly string[];
  readonly tiers: readonly string[];
  readonly epistemic: readonly string[];
  readonly validAtMs: number | null;
  readonly asOfEvent: number | null;
  readonly asOfMs: number | null;
  readonly mode: SearchMode;
  readonly expand: SearchExpand;
  readonly rerank: boolean;
  readonly debug: boolean;
  readonly minSeq: number | null;
  readonly cursor: string | null;
  readonly limit: number;
  readonly budgetTokens: number;
}

const INSTANT = /^(\d{4})-(\d{2})-(\d{2})(?:[T ](\d{2}):(\d{2})(?::(\d{2}))?Z?)?$/;

/** A date or a date and time, in UTC. Null for anything else, including a date the calendar doesn't have. */
export const parseInstant = (raw: string): number | null => {
  const match = INSTANT.exec(raw.trim());
  if (match === null) return null;
  const [year, month, day, hour, minute, second] = match.slice(1).map((part) => (part === undefined ? 0 : Number(part))) as [number, number, number, number, number, number];
  const leap = year % 4 === 0 && (year % 100 !== 0 || year % 400 === 0);
  const days = [31, leap ? 29 : 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31][month - 1];
  // `Date.UTC` rolls 31 February over to March, so what the calendar doesn't have is refused first.
  if (days === undefined || day < 1 || day > days || hour > 23 || minute > 59 || second > 59) return null;
  return Date.UTC(year, month - 1, day, hour, minute, second);
};

export type AsOf = { readonly kind: "event"; readonly seq: number } | { readonly kind: "time"; readonly ms: number };

/** An event's number, or a date and time. */
export const parseAsOf = (raw: string): AsOf | null => {
  const text = raw.trim();
  if (/^\d{1,15}$/.test(text)) return { kind: "event", seq: Number(text) };
  const ms = parseInstant(text);
  return ms === null ? null : { kind: "time", ms };
};

const listOf = (raw: string | null): readonly string[] => (raw === null ? [] : [...new Set(raw.split(",").map((v) => v.trim()).filter((v) => v !== ""))]);

/** The API refuses these together, as a past state is read from the log, which has no vectors, reranker or index to wait for. */
const consistent = (state: SearchState): SearchState => (state.asOf === "" ? state : { ...state, rerank: false, minSeq: null, mode: state.mode === "vector" ? "hybrid" : state.mode });

export interface ParsedSearch {
  readonly state: SearchState;
  /** The URL's parameters that couldn't be used, so a screen can say so rather than quietly showing something else. */
  readonly ignored: readonly string[];
}

/** Reads a URL's query. Whatever anyone typed or a broken link holds, this gives a usable state and never throws. */
export const parseSearchState = (query: (name: string) => string | null): ParsedSearch => {
  const ignored: string[] = [];
  const known = (name: string, allowed: readonly string[]): readonly string[] => {
    const values = listOf(query(name));
    const good = values.filter((v) => allowed.includes(v));
    if (good.length !== values.length) ignored.push(name);
    return good;
  };
  const free = (name: string): readonly string[] => {
    const values = listOf(query(name));
    if (values.length > MAX_VALUES) ignored.push(name);
    return values.slice(0, MAX_VALUES);
  };
  const oneOf = <T extends string>(name: string, allowed: readonly T[], fallback: T): T => {
    const raw = query(name);
    if (raw === null) return fallback;
    const found = allowed.find((a) => a === raw);
    if (found === undefined) ignored.push(name);
    return found ?? fallback;
  };
  const flag = (name: string): boolean => {
    const raw = query(name);
    if (raw !== null && raw !== "1") ignored.push(name);
    return raw === "1";
  };
  const readable = (name: string, reads: (raw: string) => unknown): string => {
    const raw = query(name);
    if (raw === null || raw.trim() === "") return "";
    if (reads(raw) === null) {
      ignored.push(name);
      return "";
    }
    return raw.trim();
  };
  const whole = (name: string, min: number, max: number): number | null => {
    const raw = query(name);
    if (raw === null) return null;
    const n = /^\d{1,15}$/.test(raw) ? Number(raw) : Number.NaN;
    if (Number.isNaN(n) || n < min || n > max) {
      ignored.push(name);
      return null;
    }
    return n;
  };
  const rawQ = query("q") ?? "";
  // A NUL can't reach Postgres, and a query this long is a broken link, not a question.
  const usableQ = !rawQ.includes("\u0000") && rawQ.length <= MAX_QUERY_CHARS;
  if (!usableQ) ignored.push("q");
  const cursor = query("cursor");
  const state: SearchState = {
    q: usableQ ? rawQ : "",
    kinds: known("kind", SEARCH_KINDS),
    types: free("type"),
    sources: free("source"),
    tiers: known("tier", TRUST_TIERS),
    epistemic: known("epistemic", SEARCH_EPISTEMIC),
    validAt: readable("valid_at", parseInstant),
    asOf: readable("as_of", parseAsOf),
    mode: oneOf("mode", SEARCH_MODES, "hybrid"),
    expand: oneOf("expand", SEARCH_EXPANDS, "auto"),
    rerank: flag("rerank"),
    debug: flag("debug"),
    minSeq: whole("min_seq", 0, Number.MAX_SAFE_INTEGER),
    cursor: cursor === null || cursor === "" ? null : cursor,
    limit: whole("limit", 1, 100) ?? SEARCH_DEFAULT_LIMIT,
  };
  const fixed = consistent(state);
  if (fixed.rerank !== state.rerank) ignored.push("rerank");
  if (fixed.minSeq !== state.minSeq) ignored.push("min_seq");
  if (fixed.mode !== state.mode) ignored.push("mode");
  return { state: fixed, ignored };
};

const list = (values: readonly string[]): string | null => (values.length === 0 ? null : values.join(","));
const notEmpty = (value: string): string | null => (value === "" ? null : value);

/** The address of a state. Defaults are left out, so the plainest search has the plainest link. */
export const searchPath = (vault: VaultId, state: SearchState): string => {
  const params: readonly (readonly [string, string | null])[] = [
    ["q", notEmpty(state.q)],
    ["kind", list(state.kinds)],
    ["type", list(state.types)],
    ["source", list(state.sources)],
    ["tier", list(state.tiers)],
    ["epistemic", list(state.epistemic)],
    ["valid_at", notEmpty(state.validAt)],
    ["as_of", notEmpty(state.asOf)],
    ["mode", state.mode === "hybrid" ? null : state.mode],
    ["expand", state.expand === "auto" ? null : state.expand],
    ["rerank", state.rerank ? "1" : null],
    ["debug", state.debug ? "1" : null],
    ["min_seq", state.minSeq === null ? null : String(state.minSeq)],
    ["limit", state.limit === SEARCH_DEFAULT_LIMIT ? null : String(state.limit)],
    ["cursor", state.cursor],
  ];
  const query = params.flatMap(([name, value]) => (value === null ? [] : [`${name}=${encodeURIComponent(value)}`])).join("&");
  const root = sectionPath(vault, "search");
  return query === "" ? root : `${root}?${query}`;
};

/** Any change but turning the page starts from the first result, as a cursor is a rank in one particular answer. */
export const changeSearch = (state: SearchState, change: Partial<SearchState>): SearchState => consistent({ ...state, cursor: null, ...change });

export const toggled = (values: readonly string[], value: string): readonly string[] => (values.includes(value) ? values.filter((v) => v !== value) : [...values, value]);

/** Null when there's nothing to search for, which is never sent. The text otherwise goes as it was typed, so the API alone decides what's an identifier. */
export const searchRequestOf = (state: SearchState): SearchRequest | null => {
  if (state.q.trim() === "") return null;
  const asOf = state.asOf === "" ? null : parseAsOf(state.asOf);
  return {
    q: state.q,
    kinds: state.kinds,
    types: state.types,
    sources: state.sources,
    tiers: state.tiers,
    epistemic: state.epistemic,
    validAtMs: state.validAt === "" ? null : parseInstant(state.validAt),
    asOfEvent: asOf?.kind === "event" ? asOf.seq : null,
    asOfMs: asOf?.kind === "time" ? asOf.ms : null,
    mode: state.mode,
    expand: state.expand,
    rerank: state.rerank,
    debug: state.debug,
    minSeq: state.minSeq,
    cursor: state.cursor,
    limit: state.limit,
    budgetTokens: state.limit * TOKENS_PER_ITEM,
  };
};

export interface MarkedPart {
  readonly text: string;
  readonly marked: boolean;
}

const MAX_TERMS = 32;
const MIN_TERM_CHARS = 3;
const escapeRegex = (text: string): string => text.replace(/[.*+?^${}()|[\]\\/-]/g, "\\$&");

/** The words and quoted phrases of a query, longest first, so a phrase wins over a word inside it. */
export const queryTerms = (q: string): readonly string[] => {
  const terms: string[] = [];
  const unquoted = q.replace(/"([^"]*)"/g, (_, phrase: string) => {
    if (phrase.trim() !== "") terms.push(phrase.trim());
    return " ";
  });
  terms.push(...unquoted.split(/\s+/).map((w) => w.replace(/^[^\p{L}\p{N}]+|[^\p{L}\p{N}]+$/gu, "")).filter((w) => w !== ""));
  const useful = terms.filter((t) => [...t].length >= MIN_TERM_CHARS);
  return [...new Set(useful.length > 0 ? useful : terms)].sort((a, b) => b.length - a.length).slice(0, MAX_TERMS);
};

const partsOf = (text: string, pattern: RegExp): readonly MarkedPart[] => {
  const parts: MarkedPart[] = [];
  let at = 0;
  for (const match of text.matchAll(pattern)) {
    if (match[0] === "") continue;
    if (match.index > at) parts.push({ text: text.slice(at, match.index), marked: false });
    parts.push({ text: match[0], marked: true });
    at = match.index + match[0].length;
  }
  if (at < text.length) parts.push({ text: text.slice(at), marked: false });
  return parts;
};

/** Splits a text at the query's words. Put back together the parts are exactly the text, as it's what a person is asked to trust. */
export const markTerms = (text: string, terms: readonly string[]): readonly MarkedPart[] => {
  if (terms.length === 0) return text === "" ? [] : [{ text, marked: false }];
  return partsOf(text, new RegExp(`(?<![\\p{L}\\p{N}])(?:${terms.map(escapeRegex).join("|")})`, "giu"));
};

/** Splits a text at a quote, which matches apart from runs of whitespace, as the API compares them. */
export const markQuote = (text: string, quote: string): readonly MarkedPart[] => {
  const words = quote.split(/\s+/).filter((w) => w !== "");
  if (words.length === 0) return text === "" ? [] : [{ text, marked: false }];
  const found = new RegExp(words.map(escapeRegex).join("\\s+"), "u").exec(text);
  if (found === null) return [{ text, marked: false }];
  const end = found.index + found[0].length;
  return [{ text: text.slice(0, found.index), marked: false }, { text: found[0], marked: true }, { text: text.slice(end), marked: false }].filter((p) => p.text !== "");
};

export interface ItemGroup {
  readonly kind: string;
  readonly label: string;
  readonly items: readonly EvidenceItem[];
}

const GROUPS: readonly (readonly [string, string])[] = [["claim", "Claims"], ["entity", "Entities"], ["page", "Pages"], ["block", "Page sections"], ["segment", "Passages"], ["section", "Surrounding sections"]];

/** Claims first, then the rest in the package's terms, each keeping the API's order. A kind from a newer server still shows, after the others. */
export const groupItems = (items: readonly EvidenceItem[]): readonly ItemGroup[] => {
  const known = GROUPS.map(([kind, label]) => ({ kind, label, items: items.filter((i) => i.kind === kind) }));
  const others = [...new Set(items.map((i) => i.kind))].filter((k) => !GROUPS.some(([kind]) => kind === k)).map((kind) => ({ kind, label: kind, items: items.filter((i) => i.kind === kind) }));
  return [...known, ...others].filter((g) => g.items.length > 0);
};

const sourceRoute = (id: string, over: Partial<SourceViewRoute>): SourceViewRoute => ({ kind: "source", id, tab: "view", page: null, zoom: null, claim: null, evidence: null, fromMs: null, toMs: null, rendition: null, segment: null, ...over });

/** The exact place a citation names. Null when it names no source, such as a claim whose source was erased. */
export const citationPath = (vault: VaultId, item: Pick<EvidenceItem, "id" | "kind">, citation: SearchCitation): string | null => {
  if (citation.sourceId === null) return null;
  const firstRect = citation.rects[0];
  const page = firstRect?.page ?? citation.pageFrom;
  const time = citation.tStartMs === null ? {} : { fromMs: citation.tStartMs, toMs: citation.tEndMs };
  // Only a claim's own evidence has a lineage to read. An entity's citation is evidence of a claim this item doesn't name.
  if (item.kind === "claim" && citation.evidenceId !== null) return documentPath(vault, sourceRoute(citation.sourceId, { page, claim: item.id, evidence: citation.evidenceId, ...time }));
  return documentPath(vault, sourceRoute(citation.sourceId, { page, segment: citation.segmentId, ...time }));
};

/** Where an item itself lives, when that isn't a source. */
export const itemPath = (vault: VaultId, item: Pick<EvidenceItem, "id" | "kind" | "pageId">): string | null => {
  if ((item.kind === "page" || item.kind === "block") && item.pageId !== null) return sectionPath(vault, "pages", item.pageId);
  if (item.kind === "entity") return sectionPath(vault, "entities", item.id);
  if (item.kind === "claim") return documentPath(vault, { kind: "lineage", claim: item.id, evidence: null });
  return null;
};

export type SearchProblem =
  /** The sequence asked for is past the end of the log, and will not arrive by waiting. */
  | { readonly kind: "min_seq_ahead"; readonly minSeq: number }
  /** The indexes haven't reached it yet, which time fixes. */
  | { readonly kind: "not_caught_up" }
  /** The cursor isn't one the API can read. */
  | { readonly kind: "bad_cursor" }
  /** Too many past-state searches are running. */
  | { readonly kind: "busy" }
  | { readonly kind: "other" };

/** The cases a person can do something about. Everything else is the shared error state. */
export const searchProblem = (failure: ApiFailure, state: SearchState): SearchProblem => {
  if (failure.kind !== "http") return { kind: "other" };
  if (failure.code === "not_caught_up") return { kind: "not_caught_up" };
  if (failure.status === 429) return { kind: "busy" };
  if (failure.status === 400 && state.cursor !== null && /cursor/i.test(failure.message)) return { kind: "bad_cursor" };
  if (failure.status === 400 && state.minSeq !== null && /min_seq/i.test(failure.message)) return { kind: "min_seq_ahead", minSeq: state.minSeq };
  return { kind: "other" };
};
