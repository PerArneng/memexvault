import { epochMs } from "@memex/types";
import type { EntityCandidate, EntityDetail, EntityFieldDetail, EntityRedirect, EntityResolution, EntitySummary, EntityType, EntityTypePreview, FieldClaim, FieldEvidence, Neighbor, Page, SameAsSuggestion } from "@memex/types";
import type { components } from "./generated/schema";

type Schemas = components["schemas"];

const isRecord = (value: unknown): value is Readonly<Record<string, unknown>> => typeof value === "object" && value !== null && !Array.isArray(value);
const pageOf = <W, T>(items: readonly W[], next: string | null | undefined, one: (w: W) => T): Page<T> => ({ items: items.map(one), nextCursor: next ?? null });

export const entityTypeFromWire = (t: Schemas["EntityTypeDto"]): EntityType => ({
  name: t.name,
  version: t.version,
  schema: t.schema,
  change: t.change,
  reasons: t.reasons,
  changesetId: t.changeset_id ?? null,
  createdAt: epochMs(t.created_at_ms),
});
export const entityTypePageFromWire = (p: Schemas["EntityTypePage"]): Page<EntityType> => pageOf(p.items, p.next_cursor, entityTypeFromWire);

export const entityTypePreviewFromWire = (p: Schemas["TypePreviewDto"]): EntityTypePreview => ({
  problem: p.problem ?? null,
  change: p.change,
  reasons: p.reasons,
  currentVersion: p.current_version ?? null,
  nextVersion: p.next_version,
  decision: p.decision,
});

export const entitySummaryFromWire = (e: Schemas["EntityDto"]): EntitySummary => ({
  id: e.id,
  typeName: e.type,
  typeVersion: e.type_version,
  version: e.version,
  aliases: e.aliases,
  attributes: isRecord(e.attributes) ? e.attributes : {},
  conflictedFields: e.conflicted_fields,
  mergedFrom: e.merged_from,
  updatedAt: epochMs(e.updated_at_ms),
});
export const entityPageFromWire = (p: Schemas["EntityPage"]): Page<EntitySummary> => pageOf(p.items, p.next_cursor, entitySummaryFromWire);

const evidenceFromWire = (e: Schemas["FieldEvidenceDto"]): FieldEvidence => ({
  evidenceId: e.evidence_id,
  quote: e.quote,
  stance: e.stance,
  segmentId: e.segment_id,
  sourceId: e.source_id ?? null,
  sourceName: e.source_name ?? null,
  trustTier: e.trust_tier ?? null,
  renditionId: e.rendition_id ?? null,
  pageFrom: e.page_from ?? null,
  pageTo: e.page_to ?? null,
  anchorBroken: e.anchor_broken,
});

const claimFromWire = (c: Schemas["FieldClaimDto"]): FieldClaim => ({
  assertionId: c.assertion_id,
  value: c.value,
  epistemic: c.epistemic ?? null,
  machine: c.machine ?? null,
  creator: c.creator ?? null,
  changesetId: c.changeset_id ?? null,
  evidence: c.evidence.map(evidenceFromWire),
});

const fieldFromWire = (f: Schemas["EntityFieldDto"]): EntityFieldDetail => ({
  field: f.field,
  value: f.value,
  why: f.why,
  current: claimFromWire(f.current),
  conflicts: f.conflicts.map(claimFromWire),
  agreeing: f.agreeing.map(claimFromWire),
});

export const entityDetailFromWire = (e: Schemas["EntityDetailDto"]): EntityDetail => ({
  id: e.id,
  typeName: e.type,
  typeVersion: e.type_version,
  version: e.version,
  aliases: e.aliases,
  mergedFrom: e.merged_from,
  redirectedFrom: e.redirected_from ?? null,
  changesetId: e.changeset_id,
  updatedAt: epochMs(e.updated_at_ms),
  fields: e.fields.map(fieldFromWire),
});

export const entityRedirectFromWire = (r: Schemas["RedirectDto"]): EntityRedirect => ({ id: r.id, currentId: r.current_id, chain: r.chain });

const candidateFromWire = (c: Schemas["CandidateDto"]): EntityCandidate => ({ entity: entitySummaryFromWire(c.entity), matchedOn: c.matched_on ?? null, exact: c.exact, score: c.score });
export const entityResolutionFromWire = (r: Schemas["ResolveResult"]): EntityResolution => ({ candidates: r.candidates.map(candidateFromWire), degraded: r.degraded });

export const sameAsPageFromWire = (p: Schemas["SameAsPage"]): Page<SameAsSuggestion> => pageOf(p.items, p.next_cursor, (s) => ({ field: s.field, value: s.value, entityIds: s.entity_ids }));

export const neighborPageFromWire = (p: Schemas["NeighborPage"]): Page<Neighbor> =>
  pageOf(p.items, p.next_cursor, (n) => ({ edgeId: n.edge_id, kind: n.kind, direction: n.direction, from: n.from, to: n.to, neighbor: n.neighbor ?? null, title: n.title ?? null }));

/** A claim's version, for a value that replaces it. The lineage route carries the claim's stored row. */
export const claimVersionFromWire = (l: Schemas["LineageDto"]): number | null => {
  const assertion: unknown = l.assertion;
  const header: unknown = isRecord(assertion) ? assertion.header : null;
  const version: unknown = isRecord(header) ? header.version : null;
  return typeof version === "number" ? version : null;
};
