import type { Identity, VaultId, WikiPageSummary } from "@memex/types";
import { scopeIn } from "./navigation";
import { foldText } from "./wikitext";

const byTitle = (a: WikiPageSummary, b: WikiPageSummary): number => a.title.localeCompare(b.title, "en") || a.id.localeCompare(b.id, "en");

export interface RankedPage {
  readonly page: WikiPageSummary;
  /** The old slug that matched, when the query found it that way. */
  readonly viaRedirect: string | null;
}

const wordStart = (text: string, query: string): boolean => text.split(/[\s-]+/).some((w) => w.startsWith(query));

const scoreOf = (page: WikiPageSummary, query: string): { readonly score: number; readonly viaRedirect: string | null } | null => {
  const title = foldText(page.title);
  const slug = foldText(page.slug);
  if (slug.startsWith(query)) return { score: 0, viaRedirect: null };
  if (title.startsWith(query)) return { score: 1, viaRedirect: null };
  if (wordStart(title, query) || wordStart(slug, query)) return { score: 2, viaRedirect: null };
  const old = page.redirects.find((r) => foldText(r).startsWith(query));
  if (old !== undefined) return { score: 3, viaRedirect: old };
  return title.includes(query) || slug.includes(query) ? { score: 4, viaRedirect: null } : null;
};

/** The pages a `[[` can complete to, best first. With nothing typed yet, the newest changes come first, as those are the likely targets. */
export const rankPages = (pages: readonly WikiPageSummary[], typed: string, limit = 20): readonly RankedPage[] => {
  // A slug has dashes where a title has spaces, and people type either.
  const query = foldText(typed).trim().replace(/\s+/g, " ");
  if (query === "") {
    return [...pages].sort((a, b) => b.updatedAt - a.updatedAt || byTitle(a, b)).slice(0, limit).map((page) => ({ page, viaRedirect: null }));
  }
  const dashed = query.replace(/ /g, "-");
  const scored = pages.flatMap((page) => {
    const hit = scoreOf(page, query) ?? scoreOf(page, dashed);
    return hit === null ? [] : [{ page, ...hit }];
  });
  scored.sort((a, b) => a.score - b.score || byTitle(a.page, b.page));
  return scored.slice(0, limit).map(({ page, viaRedirect }) => ({ page, viaRedirect }));
};

export interface PageIndex {
  /** The page holding a slug, current or old. */
  bySlug(slug: string): WikiPageSummary | null;
}

export const pageIndex = (pages: readonly WikiPageSummary[]): PageIndex => {
  const slugs = new Map<string, WikiPageSummary>();
  for (const page of pages) {
    slugs.set(page.slug, page);
    for (const old of page.redirects) slugs.set(old, page);
  }
  return { bySlug: (slug) => slugs.get(slug) ?? null };
};

export interface TreeRow {
  readonly page: WikiPageSummary;
  readonly depth: number;
}

/**
 * Pages have no parents, as a slug is one flat word. So the tree comes from names: `alpha-notes` sits under the page `alpha` when one exists,
 * and under the longest such page when several do.
 */
export const pageTree = (pages: readonly WikiPageSummary[]): readonly TreeRow[] => {
  const slugs = new Map(pages.map((p) => [p.slug, p]));
  const children = new Map<string, WikiPageSummary[]>();
  const roots: WikiPageSummary[] = [];
  for (const page of pages) {
    let parent: WikiPageSummary | undefined;
    let cut = page.slug.lastIndexOf("-");
    while (cut > 0 && parent === undefined) {
      parent = slugs.get(page.slug.slice(0, cut));
      cut = page.slug.lastIndexOf("-", cut - 1);
    }
    if (parent === undefined) roots.push(page);
    else children.set(parent.slug, [...(children.get(parent.slug) ?? []), page]);
  }
  const rows: TreeRow[] = [];
  const add = (page: WikiPageSummary, depth: number) => {
    rows.push({ page, depth });
    for (const child of [...(children.get(page.slug) ?? [])].sort(byTitle)) add(child, depth + 1);
  };
  for (const root of [...roots].sort(byTitle)) add(root, 0);
  return rows;
};

export const filterPages = (pages: readonly WikiPageSummary[], typed: string): readonly WikiPageSummary[] => {
  const query = foldText(typed).trim();
  if (query === "") return pages;
  return pages.filter((p) => foldText(p.title).includes(query) || foldText(p.slug).includes(query));
};

/** Reading is `read`, and any write is at least a proposal, as on the server. */
export const canWritePages = (identity: Identity, vault: VaultId): boolean => {
  const scope = scopeIn(identity, vault);
  return scope === "propose" || scope === "admin";
};
