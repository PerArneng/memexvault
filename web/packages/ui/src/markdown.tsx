import { INLINE_CLOSE, INLINE_OPEN } from "@memex/types";
import { ExternalLink, ImageOff } from "lucide-react";
import { createElement, useMemo } from "react";
import type { ComponentProps, ReactNode } from "react";
import ReactMarkdown from "react-markdown";
import type { Components } from "react-markdown";
import rehypeSanitize from "rehype-sanitize";
import type { Options as SanitizeSchema } from "rehype-sanitize";
import remarkGfm from "remark-gfm";

// The one way vault content becomes HTML. See "Security" in docs/frontend-architecture.md. It holds nothing specific to pages, as chat uses it too.

const INLINE_TAG = "memex-inline";
const BLOCKED_IMAGE_TAG = "memex-blocked-image";
const ALLOWED_PROTOCOLS = ["http:", "https:", "mailto:"] as const;
/** Parses relative addresses. It isn't ever requested. */
const PARSE_BASE = "http://relative.invalid";

/** An allowlist of what Markdown itself makes, rather than the library's default with parts removed. Nothing here can carry a script or a request. */
const SCHEMA: SanitizeSchema = {
  tagNames: ["a", "blockquote", "br", "code", "del", "em", "h1", "h2", "h3", "h4", "h5", "h6", "hr", "input", "li", "ol", "p", "pre", "strong", "table", "tbody", "td", "th", "thead", "tr", "ul", INLINE_TAG, BLOCKED_IMAGE_TAG],
  attributes: {
    a: ["href", "title"],
    input: [["type", "checkbox"], ["disabled", true], "checked"],
    td: ["align"],
    th: ["align"],
    [INLINE_TAG]: ["dataIndex", "dataInLink"],
    [BLOCKED_IMAGE_TAG]: ["dataAlt", "dataSrc"],
  },
  protocols: { href: ["http", "https", "mailto"] },
  strip: ["script", "style"],
};

interface TreeNode {
  type: string;
  children?: TreeNode[];
  value?: string;
  url?: string;
  title?: string | null;
  alt?: string | null;
  tagName?: string;
  properties?: Record<string, unknown>;
  data?: Record<string, unknown>;
}

const PLACEHOLDER = new RegExp(`${INLINE_OPEN}(\\d+)${INLINE_CLOSE}`, "g");

/** Text that isn't Markdown's own: an address, a title, some code. A placeholder that ended up there goes back to what it stood for. */
const restored = (text: string, raw: (index: number) => string): string => text.replace(PLACEHOLDER, (_, index: string) => raw(Number(index)));

const splitPlaceholders = (value: string, insideLink: boolean): TreeNode[] => {
  const parts: TreeNode[] = [];
  let last = 0;
  for (const match of value.matchAll(PLACEHOLDER)) {
    if (match.index > last) parts.push({ type: "text", value: value.slice(last, match.index) });
    parts.push({ type: "memexInline", data: { hName: INLINE_TAG, hProperties: { dataIndex: match[1], ...(insideLink ? { dataInLink: "1" } : {}) } } });
    last = match.index + match[0].length;
  }
  if (last < value.length) parts.push({ type: "text", value: value.slice(last) });
  return parts;
};

/** Raw HTML shows as the text it is, so a reader can see what's on the page, and placeholders become elements. */
const remarkInlines = (raw: (index: number) => string) => () => {
  const walk = (node: TreeNode, insideLink: boolean) => {
    if (node.value !== undefined && node.type !== "text") node.value = restored(node.value, raw);
    if (node.url !== undefined) node.url = restored(node.url, raw);
    if (typeof node.title === "string") node.title = restored(node.title, raw);
    if (typeof node.alt === "string") node.alt = restored(node.alt, raw);
    if (node.children === undefined) return;
    const inner = insideLink || node.type === "link" || node.type === "linkReference";
    node.children = node.children.flatMap((child) => {
      if (child.type === "html") return [{ type: "text", value: child.value ?? "" }];
      if (child.type === "text" && child.value?.includes(INLINE_OPEN) === true) return splitPlaceholders(child.value, inner);
      return [child];
    });
    // A placeholder in raw HTML came back as text above, so it's split on this second look.
    node.children = node.children.flatMap((child) => (child.type === "text" && child.value?.includes(INLINE_OPEN) === true ? splitPlaceholders(child.value, inner) : [child]));
    for (const child of node.children) walk(child, inner);
  };
  return (tree: TreeNode) => {
    walk(tree, false);
  };
};

/** An image is never requested, as its address could carry vault data to another site. This runs before the sanitizer, so that's what sees the result. */
const rehypeBlockImages = () => {
  const walk = (node: TreeNode) => {
    for (const child of node.children ?? []) {
      if (child.type === "element" && child.tagName === "img") {
        const { alt, src } = child.properties ?? {};
        child.tagName = BLOCKED_IMAGE_TAG;
        child.properties = { dataAlt: typeof alt === "string" ? alt : "", dataSrc: typeof src === "string" ? src : "" };
      }
      walk(child);
    }
  };
  return (tree: TreeNode) => {
    walk(tree);
  };
};

/** Null for anything but web and mail addresses, which also covers `javascript:` however it's disguised. Relative ones stay on this site. */
export const classifyHref = (href: string | undefined): "internal" | "external" | null => {
  if (href === undefined || href === "") return null;
  let url: URL;
  try {
    url = new URL(href, PARSE_BASE);
  } catch {
    return null;
  }
  if (!(ALLOWED_PROTOCOLS as readonly string[]).includes(url.protocol)) return null;
  return url.origin === PARSE_BASE ? "internal" : "external";
};

const urlTransform = (url: string): string => (classifyHref(url) === null ? "" : url);

const SafeLink = ({ href, title, children }: ComponentProps<"a">) => {
  const kind = classifyHref(href);
  // What was a dangerous address is only its words now.
  if (kind === null) return <span data-blocked-link="true">{children}</span>;
  if (kind === "internal") return <a href={href} title={title} className="text-accent underline">{children}</a>;
  return (
    <a href={href} title={title} target="_blank" rel="noopener noreferrer" className="inline-flex items-baseline gap-0.5 text-accent underline">
      {children}
      <ExternalLink size={11} aria-hidden={true} className="self-center" />
      <span className="sr-only"> (external link)</span>
    </a>
  );
};

const BlockedImage = (props: Record<string, unknown>) => {
  const alt = typeof props["data-alt"] === "string" ? props["data-alt"] : "";
  const src = typeof props["data-src"] === "string" ? props["data-src"] : "";
  return (
    <span className="inline-flex max-w-full items-center gap-1 rounded-sm border border-dashed border-border-strong px-1 text-xs text-text-muted">
      <ImageOff size={12} aria-hidden={true} className="shrink-0" />
      <span className="truncate">Image not loaded{alt === "" ? "" : `: ${alt}`}{src === "" ? "" : ` (${src})`}</span>
    </span>
  );
};

const STYLED: Components = {
  a: SafeLink,
  // The screen's title and the page's title sit above, so a page's own headings start lower.
  h1: ({ children }) => <h3 className="mt-4 mb-2 text-lg font-medium first:mt-0">{children}</h3>,
  h2: ({ children }) => <h4 className="mt-4 mb-2 font-medium first:mt-0">{children}</h4>,
  h3: ({ children }) => <h5 className="mt-3 mb-1 font-medium first:mt-0">{children}</h5>,
  h4: ({ children }) => <h6 className="mt-3 mb-1 font-medium first:mt-0">{children}</h6>,
  h5: ({ children }) => <h6 className="mt-3 mb-1 text-text-muted first:mt-0">{children}</h6>,
  h6: ({ children }) => <h6 className="mt-3 mb-1 text-xs text-text-muted first:mt-0">{children}</h6>,
  p: ({ children }) => <p className="my-2 leading-relaxed first:mt-0 last:mb-0">{children}</p>,
  ul: ({ children }) => <ul className="my-2 list-disc pl-5">{children}</ul>,
  ol: ({ children }) => <ol className="my-2 list-decimal pl-5">{children}</ol>,
  blockquote: ({ children }) => <blockquote className="my-2 border-l-2 border-border-strong pl-3 text-text-muted">{children}</blockquote>,
  // It can scroll sideways, and a keyboard has to be able to do that too.
  pre: ({ children }) => <pre tabIndex={0} className="my-2 overflow-auto rounded-md bg-surface-raised p-2 font-mono text-xs">{children}</pre>,
  code: ({ children }) => <code className="rounded-sm bg-surface-raised px-0.5 font-mono text-xs">{children}</code>,
  hr: () => <hr className="my-3 border-border" />,
  table: ({ children }) => <table className="my-2 border-collapse text-sm">{children}</table>,
  th: ({ children, align }) => <th align={align} className="border border-border px-2 py-1 font-medium">{children}</th>,
  td: ({ children, align }) => <td align={align} className="border border-border px-2 py-1">{children}</td>,
};

export interface SafeMarkdownProps {
  /** Untrusted. May hold `INLINE_OPEN<n>INLINE_CLOSE` placeholders, and any it holds must have come from whatever made `renderInline`. */
  readonly source: string;
  /** Draws a placeholder. `insideLink` means it can't be a link itself, as links can't nest. */
  readonly renderInline?: (index: number, insideLink: boolean) => ReactNode;
  /** The original text of a placeholder, for one that ended up in code or an address. */
  readonly rawInline?: (index: number) => string;
}

/**
 * Markdown with raw HTML shown as text, an allowlist sanitizer, no images and only web and mail links. Nothing it renders can run a script
 * or make a request by itself, and the server's CSP stands behind it.
 */
export const SafeMarkdown = ({ source, renderInline, rawInline }: SafeMarkdownProps) => {
  const components = useMemo(() => {
    const raw = {
      ...STYLED,
      [BLOCKED_IMAGE_TAG]: BlockedImage,
      [INLINE_TAG]: (props: Record<string, unknown>) => {
        const index = Number(props["data-index"]);
        return renderInline === undefined ? null : renderInline(index, props["data-in-link"] === "1");
      },
    };
    return raw as Components;
  }, [renderInline]);
  const remarkPlugins = useMemo(() => [remarkGfm, remarkInlines(rawInline ?? (() => ""))], [rawInline]);
  return createElement(ReactMarkdown, { remarkPlugins, rehypePlugins: [rehypeBlockImages, [rehypeSanitize, SCHEMA]], components, urlTransform, children: source });
};
