import { INLINE_CLOSE, INLINE_OPEN } from "@memex/types";

/**
 * The console's copy of `memex_core::page`: slugs, blocks and inline references. They have to agree with the server, or the reader would draw
 * a link where the server has none. `wikitext.test.ts` holds the server's own cases.
 */
const MAX_SLUG_BYTES = 200;
const MAX_CLAIM_ID_BYTES = 64;

const FOLD: Readonly<Record<string, string>> = {
  å: "a", ä: "a", á: "a", à: "a", â: "a", ã: "a",
  ö: "o", ó: "o", ò: "o", ô: "o", õ: "o", ø: "o",
  ü: "u", ú: "u", ù: "u", û: "u",
  é: "e", è: "e", ê: "e", ë: "e",
  í: "i", ì: "i", î: "i", ï: "i",
  ñ: "n", ç: "c", ß: "ss",
};

/** Lower case with common diacritics gone, so `Goteborg` matches `Göteborg`. */
export const foldText = (text: string): string => [...text.toLowerCase()].map((c) => FOLD[c] ?? c).join("");

const utf8Length = (c: string): number => {
  const code = c.codePointAt(0) ?? 0;
  if (code < 0x80) return 1;
  if (code < 0x800) return 2;
  return code < 0x10000 ? 3 : 4;
};

const ALPHANUMERIC = /^[\p{Alphabetic}\p{N}]$/u;

/** A slug from any title. Runs of anything but letters and digits become one dash. Null if nothing is left. */
export const slugify = (text: string): string | null => {
  const out: string[] = [];
  for (const c of foldText(text).toLowerCase()) {
    if (ALPHANUMERIC.test(c)) out.push(c);
    else if (out.length > 0 && out[out.length - 1] !== "-") out.push("-");
  }
  while (out[out.length - 1] === "-") out.pop();
  let bytes = 0;
  let end = 0;
  for (const c of out) {
    bytes += utf8Length(c);
    if (bytes > MAX_SLUG_BYTES) break;
    end += 1;
  }
  const kept = out.slice(0, end);
  while (kept[kept.length - 1] === "-") kept.pop();
  return kept.length === 0 ? null : kept.join("");
};

const fenceOf = (line: string): { readonly char: string; readonly run: number } | null => {
  const indent = line.length - line.replace(/^ +/, "").length;
  if (indent > 3) return null;
  const rest = line.slice(indent);
  const char = rest[0];
  if (char !== "`" && char !== "~") return null;
  let run = 0;
  while (rest[run] === char) run += 1;
  if (run < 3 || (char === "`" && rest.slice(run).includes("`"))) return null;
  return { char, run };
};

const closesFence = (line: string, open: { readonly char: string; readonly run: number }): boolean => {
  const text = line.trim();
  return [...text].length >= open.run && [...text].every((c) => c === open.char) && line.length - line.replace(/^ +/, "").length <= 3;
};

const isHeading = (line: string): boolean => {
  const hashes = /^#*/.exec(line)?.[0].length ?? 0;
  const rest = line.slice(hashes);
  return hashes >= 1 && hashes <= 6 && rest.startsWith(" ") && rest.trim() !== "";
};

const isRule = (line: string): boolean => {
  const text = line.replace(/\s/g, "");
  return text.length >= 3 && ["-", "*", "_"].some((m) => [...text].every((c) => c === m));
};

export interface TextBlock {
  /** Offsets into the source. */
  readonly start: number;
  readonly end: number;
  readonly code: boolean;
}

/** The server's blocks: a blank line ends one, and a heading, a rule or a code fence is always its own. */
export const blocksOf = (source: string): readonly TextBlock[] => {
  const blocks: TextBlock[] = [];
  const lines = source.split("\n");
  const offsets: number[] = [];
  let at = 0;
  for (const line of lines) {
    offsets.push(at);
    at += line.length + 1;
  }
  const lineEnd = (i: number): number => (offsets[i] ?? 0) + (lines[i]?.length ?? 0);
  let open: number | null = null;
  const flush = (lastLine: number) => {
    if (open !== null) blocks.push({ start: offsets[open] ?? 0, end: lineEnd(lastLine), code: false });
    open = null;
  };
  for (let i = 0; i < lines.length; i += 1) {
    const line = (lines[i] ?? "").trimEnd();
    if (line === "") {
      flush(i - 1);
      continue;
    }
    const fence = fenceOf(line);
    if (fence !== null) {
      flush(i - 1);
      let last = lines.length - 1;
      for (let j = i + 1; j < lines.length; j += 1) {
        if (closesFence(lines[j] ?? "", fence)) {
          last = j;
          break;
        }
      }
      blocks.push({ start: offsets[i] ?? 0, end: lineEnd(last), code: true });
      i = last;
    } else if (isHeading(line) || isRule(line)) {
      flush(i - 1);
      blocks.push({ start: offsets[i] ?? 0, end: lineEnd(i), code: false });
    } else {
      open ??= i;
    }
  }
  flush(lines.length - 1);
  return blocks;
};

export type WikiToken =
  | { readonly kind: "link"; readonly target: string; readonly slug: string; readonly label: string | null; readonly raw: string }
  | { readonly kind: "claim"; readonly id: string; readonly raw: string };

const claimIdIsValid = (id: string): boolean => id !== "" && id.length <= MAX_CLAIM_ID_BYTES && /^[A-Za-z0-9_-]+$/.test(id);

/** The link starting after `[[`, and how much of the text it used. */
const parseLink = (afterOpen: string): { readonly token: WikiToken; readonly used: number } | null => {
  const end = afterOpen.indexOf("]]");
  if (end < 0) return null;
  const inner = afterOpen.slice(0, end);
  if (inner.includes("\n") || inner.includes("[")) return null;
  const bar = inner.indexOf("|");
  const target = (bar < 0 ? inner : inner.slice(0, bar)).trim();
  const label = bar < 0 ? null : inner.slice(bar + 1).trim();
  const slug = slugify(target);
  if (slug === null) return null;
  return { token: { kind: "link", target, slug, label: label === "" ? null : label, raw: `[[${inner}]]` }, used: end + 2 };
};

interface InlineVisitor {
  readonly token: (token: WikiToken, start: number, end: number) => void;
  /** Reached the end of the text inside something still open. */
  readonly open?: (kind: "link" | "claim", contentStart: number) => void;
  /** Inside a code span that closes after the end of the text. */
  readonly code?: () => void;
}

/** Walks a block's text as the server's `parse_inline` does: escapes and code spans are skipped, and anything malformed stays text. */
const walkInline = (text: string, visit: InlineVisitor, wholeBlock = text): void => {
  let i = 0;
  while (i < text.length) {
    const rest = text.slice(i);
    if (text[i] === "\\" && (rest.startsWith("\\[[") || rest.startsWith("\\{{"))) {
      i += 3;
    } else if (text[i] === "`") {
      const close = text.indexOf("`", i + 1);
      if (close < 0) {
        if (wholeBlock.indexOf("`", i + 1) >= 0) visit.code?.();
        i += 1;
      } else {
        i = close + 1;
      }
    } else if (rest.startsWith("[[")) {
      const link = parseLink(rest.slice(2));
      if (link === null) {
        if (!rest.includes("]]")) {
          // The last opener is the one being typed.
          visit.open?.("link", i + rest.lastIndexOf("[[") + 2);
          return;
        }
        i += 2;
      } else {
        visit.token(link.token, i, i + 2 + link.used);
        i += 2 + link.used;
      }
    } else if (rest.startsWith("{{claim:")) {
      const after = rest.slice(8);
      const end = after.indexOf("}}");
      const id = end < 0 ? "" : after.slice(0, end);
      if (claimIdIsValid(id)) {
        visit.token({ kind: "claim", id, raw: `{{claim:${id}}}` }, i, i + 8 + end + 2);
        i += 8 + end + 2;
      } else {
        if (end < 0) {
          visit.open?.("claim", i + 8);
          return;
        }
        i += 2;
      }
    } else {
      i += 1;
    }
  }
};

export interface PreparedText {
  /** The Markdown, with each reference replaced by a placeholder `SafeMarkdown` can find. */
  readonly text: string;
  readonly tokens: readonly WikiToken[];
}

const STRAY_MARKS = new RegExp(`[${INLINE_OPEN}${INLINE_CLOSE}]`, "g");
const normalNewlines = (source: string): string => source.replace(/\r\n?/g, "\n");

/**
 * Takes the references out before Markdown sees them, so the rules of what's a link are the server's. A Markdown parser would otherwise read
 * `[[a|b]]` in a table as two cells, and see a link reference in it whenever the page defines `[a|b]`.
 */
export const prepareWikiText = (source: string): PreparedText => {
  const clean = normalNewlines(source).replace(STRAY_MARKS, "�");
  const tokens: WikiToken[] = [];
  let out = "";
  let copied = 0;
  for (const block of blocksOf(clean)) {
    if (block.code) continue;
    walkInline(clean.slice(block.start, block.end), {
      token: (token, start, end) => {
        out += clean.slice(copied, block.start + start);
        out += `${INLINE_OPEN}${String(tokens.length)}${INLINE_CLOSE}`;
        tokens.push(token);
        copied = block.start + end;
      },
    });
  }
  return { text: out + clean.slice(copied), tokens };
};

/** The ids a body's claim references name, once each, in order. */
export const claimIdsIn = (source: string): readonly string[] => [
  ...new Set(prepareWikiText(source).tokens.flatMap((t) => (t.kind === "claim" ? [t.id] : []))),
];

export interface OpenReference {
  readonly kind: "link" | "claim";
  /** Where what's been typed starts. */
  readonly from: number;
  readonly query: string;
}

/** What the cursor is inside, or null where a reference wouldn't count: in code, after an escape, or past a link's `|`. */
export const openReferenceAt = (source: string, position: number): OpenReference | null => {
  const clean = source.slice(0, position);
  // The blocks of the whole text, since a fence opened above decides what this line is.
  const block = blocksOf(source).find((b) => b.start <= position && position <= b.end);
  if (block === undefined || block.code) return null;
  const before = clean.slice(block.start);
  let found: { readonly kind: "link" | "claim"; readonly from: number } | null = null;
  let inCode = false;
  walkInline(
    before,
    {
      token: () => undefined,
      open: (kind, contentStart) => {
        found = { kind, from: block.start + contentStart };
      },
      code: () => {
        inCode = true;
      },
    },
    source.slice(block.start, block.end),
  );
  if (found === null || inCode) return null;
  const { kind, from } = found as { readonly kind: "link" | "claim"; readonly from: number };
  const query = clean.slice(from);
  if (query.includes("\n")) return null;
  if (kind === "link" && (query.includes("|") || query.includes("["))) return null;
  return { kind, from, query };
};

/** What to put in place of the typed part, leaving out a closer that's already there. */
export const completionText = (kind: "link" | "claim", value: string, textAfterCursor: string): string => {
  const closer = kind === "link" ? "]]" : "}}";
  return textAfterCursor.startsWith(closer) ? value : `${value}${closer}`;
};
