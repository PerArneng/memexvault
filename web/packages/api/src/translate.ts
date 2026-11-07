import { epochMs, vaultId } from "@memex/types";
import type {
  ApiFailure,
  Identity,
  ObjectSummary,
  Page,
  Vault,
  WikiAttribution,
  WikiBacklink,
  WikiCitation,
  WikiClaim,
  WikiDiff,
  WikiOutgoingLink,
  WikiPage,
  WikiPageSummary,
  WikiRedirect,
  WikiRevision,
  WikiUnresolvedLink,
  WikiWrite,
} from "@memex/types";
import type { components } from "./generated/schema";

type Schemas = components["schemas"];

export const vaultFromWire = (v: Schemas["VaultDto"]): Vault => ({ id: vaultId(v.id), name: v.name, createdAt: epochMs(v.created_at_ms) });

export const vaultPageFromWire = (p: Schemas["VaultPage"]): Page<Vault> => ({ items: p.items.map(vaultFromWire), nextCursor: p.next_cursor ?? null });

export const identityFromWire = (w: Schemas["WhoAmI"]): Identity => ({
  principal: { id: w.principal.id, name: w.principal.name, serverAdmin: w.principal.server_admin },
  tokenId: w.token_id,
  grants: w.grants.map((g) => ({ vault: vaultId(g.vault_id), scope: g.scope })),
});

export interface ObjectPages {
  readonly sources: Schemas["SourcePage"];
  readonly pages: Schemas["PageSummaryPage"];
  readonly entityTypes: Schemas["EntityTypePage"];
}

export const objectsFromWire = ({ sources, pages, entityTypes }: ObjectPages): readonly ObjectSummary[] => [
  ...sources.items.map((s) => ({ kind: "document", id: s.id, label: s.name }) as const),
  ...pages.items.map((p) => ({ kind: "page", id: p.id, label: p.title }) as const),
  ...entityTypes.items.map((t) => ({ kind: "entity_type", id: t.name, label: t.name }) as const),
];

export interface WireHeaders {
  get(name: string): string | null;
}

const isErrorBody = (body: unknown): body is Schemas["ErrorBody"] => {
  if (typeof body !== "object" || body === null || !("error" in body)) return false;
  const { error } = body;
  return typeof error === "object" && error !== null && "code" in error && "message" in error && "request_id" in error;
};

const retryAfter = (headers: WireHeaders): number | undefined => {
  const raw = headers.get("retry-after");
  const seconds = raw === null ? Number.NaN : Number(raw);
  return Number.isInteger(seconds) && seconds >= 0 ? seconds : undefined;
};

/** Anything that isn't the server's error format, such as a proxy's page, stays `protocol` rather than being trusted as ours. */
export const failureFromWire = (status: number, body: unknown, headers: WireHeaders): ApiFailure => {
  if (!isErrorBody(body)) return { kind: "protocol", status, message: `The server answered ${String(status)} with something unexpected.` };
  const seconds = retryAfter(headers);
  const { code, message, request_id: requestId } = body.error;
  return { kind: "http", status, code, message, requestId, ...(seconds === undefined ? {} : { retryAfterS: seconds }) };
};

const pageOf = <W, T>(items: readonly W[], next: string | null | undefined, translate: (item: W) => T): Page<T> => ({ items: items.map(translate), nextCursor: next ?? null });

export const wikiSummaryFromWire = (p: Schemas["PageSummaryDto"]): WikiPageSummary => ({
  id: p.id,
  title: p.title,
  slug: p.slug,
  redirects: p.redirects,
  version: p.version,
  creator: p.creator,
  updatedAt: epochMs(p.updated_at_ms),
});
export const wikiSummaryPageFromWire = (p: Schemas["PageSummaryPage"]): Page<WikiPageSummary> => pageOf(p.items, p.next_cursor, wikiSummaryFromWire);

const citationFromWire = (e: Schemas["FieldEvidenceDto"]): WikiCitation => ({
  evidenceId: e.evidence_id,
  quote: e.quote,
  stance: e.stance,
  sourceId: e.source_id ?? null,
  sourceName: e.source_name ?? null,
  pageFrom: e.page_from ?? null,
  pageTo: e.page_to ?? null,
  trustTier: e.trust_tier ?? null,
  anchorBroken: e.anchor_broken,
});

/** A claim's value is any JSON. A string shows as itself, and anything else as its JSON. */
const valueText = (value: unknown): string | null => {
  if (value === null || value === undefined) return null;
  return typeof value === "string" ? value : JSON.stringify(value);
};

const claimStatus = (status: string): WikiClaim["status"] => (status === "current" || status === "superseded" ? status : "missing");

export const wikiClaimFromWire = (c: Schemas["PageClaimDto"]): WikiClaim => ({
  claimId: c.claim_id,
  status: claimStatus(c.status),
  currentClaimId: c.current_claim_id ?? null,
  subject: c.subject ?? null,
  predicate: c.predicate ?? null,
  value: valueText(c.value),
  epistemic: c.epistemic ?? null,
  citations: c.citations.map(citationFromWire),
});

export const wikiPageFromWire = (p: Schemas["PageDetailDto"]): WikiPage => ({
  id: p.id,
  currentRevision: p.current_revision,
  currentSlug: p.current_slug,
  redirects: p.redirects,
  revision: p.revision,
  title: p.title,
  slug: p.slug,
  body: p.body,
  claims: p.claims.map(wikiClaimFromWire),
  creator: p.creator,
  updatedAt: epochMs(p.updated_at_ms),
  restoredFrom: p.restored_from ?? null,
});

const revisionFromWire = (r: Schemas["RevisionDto"]): WikiRevision => ({
  revision: r.revision,
  title: r.title,
  slug: r.slug,
  creator: r.creator,
  changesetId: r.changeset_id,
  createdAt: epochMs(r.created_at_ms),
  restoredFrom: r.restored_from ?? null,
});
export const wikiRevisionPageFromWire = (p: Schemas["RevisionPage"]): Page<WikiRevision> => pageOf(p.items, p.next_cursor, revisionFromWire);

export const wikiDiffFromWire = (d: Schemas["PageDiffDto"]): WikiDiff => ({
  from: d.from,
  to: d.to,
  title: d.title ?? null,
  slug: d.slug ?? null,
  blocks: d.blocks.map((b) => ({ blockId: b.block_id, change: b.change, before: b.before ?? null, after: b.after ?? null })),
});

const linkKind = (kind: string): WikiOutgoingLink["kind"] => (kind === "claim_ref" ? "claim_ref" : "wiki_link");

export const wikiBacklinkPageFromWire = (p: Schemas["BacklinkPage"]): Page<WikiBacklink> =>
  pageOf(p.items, p.next_cursor, (b) => ({ edgeId: b.edge_id, fromPage: b.from_page, fromTitle: b.from_title ?? null, fromSlug: b.from_slug ?? null, viaSlug: b.via_slug }));

export const wikiOutgoingPageFromWire = (p: Schemas["OutgoingLinkPage"]): Page<WikiOutgoingLink> =>
  pageOf(p.items, p.next_cursor, (l) => ({ edgeId: l.edge_id, kind: linkKind(l.kind), target: l.target, toPage: l.to_page ?? null, resolved: l.resolved }));

export const wikiUnresolvedPageFromWire = (p: Schemas["UnresolvedLinkPage"]): Page<WikiUnresolvedLink> =>
  pageOf(p.items, p.next_cursor, (u) => ({ edgeId: u.edge_id, kind: linkKind(u.kind), fromPage: u.from_page, target: u.target }));

export const wikiRedirectFromWire = (r: Schemas["PageRedirectDto"]): WikiRedirect => ({ slug: r.slug, pageId: r.page_id, currentSlug: r.current_slug, redirected: r.redirected });

const isRecord = (value: unknown): value is Readonly<Record<string, unknown>> => typeof value === "object" && value !== null;

/** The document types a validator's result as any object, and each has `validator`, `passed` and maybe `detail`. */
const refusalsIn = (validators: readonly unknown[]): readonly string[] =>
  validators.flatMap((v) => {
    if (!isRecord(v) || v.passed !== false) return [];
    const name = typeof v.validator === "string" ? v.validator : "a validator";
    return [typeof v.detail === "string" && v.detail !== "" ? v.detail : `${name} refused this`];
  });

export const wikiWriteFromWire = (r: Schemas["ProposalResult"]): WikiWrite => {
  const state = r.changeset?.state;
  return {
    changesetId: r.changeset?.id ?? null,
    // No changeset is a dry run or a refusal, and neither changed anything.
    state: state === "committed" || state === "pending" ? state : "rejected",
    replayed: r.replayed,
    refusals: refusalsIn(r.validators),
  };
};

export const wikiAttributionFromWire = (c: Schemas["ChangeSetDto"]): WikiAttribution => ({
  proposer: c.proposer,
  machine: c.proposer_is_machine,
  extractor: c.extractor ?? null,
  rationale: c.rationale,
  state: c.state,
});

/**
 * A claim named by a draft isn't on any page yet, so it has only the lineage route to read. These fields are the row's own, and the page
 * route reports the same ones. A row's epistemic status serializes in lower case.
 */
export interface LineageStep {
  readonly supersededBy: string | null;
  readonly claim: WikiClaim;
}

export const lineageStepFromWire = (asked: string, l: Schemas["LineageDto"]): LineageStep => {
  const row: Readonly<Record<string, unknown>> = isRecord(l.assertion) ? l.assertion : {};
  const text = (key: string): string | null => {
    const value = row[key];
    return typeof value === "string" ? value : null;
  };
  const pages = (rects: readonly { readonly page: number }[]) => rects.map((r) => r.page);
  return {
    supersededBy: text("superseded_by"),
    claim: {
      claimId: asked,
      status: "current",
      currentClaimId: asked,
      subject: text("subject"),
      predicate: text("predicate"),
      value: valueText(row.value),
      epistemic: text("epistemic"),
      citations: l.evidence.map((e) => ({
        evidenceId: e.evidence_id,
        quote: e.quote,
        stance: e.stance,
        sourceId: e.source_id,
        sourceName: e.source_name,
        pageFrom: e.rects.length === 0 ? null : Math.min(...pages(e.rects)),
        pageTo: e.rects.length === 0 ? null : Math.max(...pages(e.rects)),
        trustTier: e.trust_tier,
        anchorBroken: e.broken,
      })),
    },
  };
};
