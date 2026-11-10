import type { EntityDetail, EntityRedirect, EntityResolution, EntitySummary, EntityType, EntityTypePreview, Neighbor, Page, SameAsSuggestion, VaultId, WikiWrite } from "@memex/types";
import { failed, ok } from "./api";
import type { Api, ApiFailure, ApiResult, EntityFieldInput } from "./api";
import { graphClaims } from "./entity-draft";
import { entityFilterText } from "./entity-routes";
import type { EntityFilter } from "./entity-schema";
import type { IdSource } from "./wiki-cases";

/** What a proposal about types or entities became. The API's word is the only source: nothing here assumes a write landed. */
export type EntityProposal =
  | { readonly kind: "saved"; readonly changesetId: string | null; readonly entityId?: string }
  /** A person has to approve it in Review, so nothing has changed yet. */
  | { readonly kind: "pending"; readonly changesetId: string | null; readonly entityId?: string }
  | { readonly kind: "refused"; readonly reasons: readonly string[] }
  /** Someone changed it first, and nothing was done. */
  | { readonly kind: "stale" }
  | { readonly kind: "failed"; readonly failure: ApiFailure };

export interface EntityTypeList {
  /** The latest version of each. */
  readonly types: readonly EntityType[];
  readonly truncated: boolean;
}

export interface EntityGraphAnswer {
  readonly answers: readonly { readonly viaField: string | null; readonly items: readonly Neighbor[] }[];
  /** The entity has more claims than the panel asks about, or an object has more links than one page. */
  readonly truncated: boolean;
  /** How many of the asks failed, which the panel says rather than showing a graph that only looks complete. */
  readonly failures: number;
}

export interface EntityCases {
  listEntityTypes(vault: VaultId): Promise<ApiResult<EntityTypeList>>;
  listEntityTypeVersions(vault: VaultId, name: string): Promise<ApiResult<readonly EntityType[]>>;
  previewEntityType(vault: VaultId, name: string, schema: unknown): Promise<ApiResult<EntityTypePreview>>;
  submitEntityType(vault: VaultId, input: { readonly name: string; readonly baseVersion: number | null; readonly schema: unknown; readonly rationale: string }): Promise<EntityProposal>;
  listEntities(vault: VaultId, type: string, filters: readonly EntityFilter[], cursor: string | null): Promise<ApiResult<Page<EntitySummary>>>;
  getEntity(vault: VaultId, id: string): Promise<ApiResult<EntityDetail>>;
  getEntityRedirect(vault: VaultId, id: string): Promise<ApiResult<EntityRedirect>>;
  resolveEntities(vault: VaultId, mention: string, type: string | null): Promise<ApiResult<EntityResolution>>;
  listSameAs(vault: VaultId, type: string): Promise<ApiResult<readonly SameAsSuggestion[]>>;
  entityNeighborhood(vault: VaultId, entity: EntityDetail): Promise<ApiResult<EntityGraphAnswer>>;
  createEntity(vault: VaultId, input: { readonly type: string; readonly aliases: readonly string[]; readonly fields: readonly EntityFieldInput[]; readonly rationale: string }): Promise<EntityProposal>;
  /** `replacing` names the claims the new values replace, by field. Their versions are read here, as late as they can be. */
  editEntity(vault: VaultId, id: string, input: { readonly baseVersion: number; readonly aliases: readonly string[] | null; readonly fields: readonly EntityFieldInput[]; readonly replacing: Readonly<Record<string, string>>; readonly rationale: string }): Promise<EntityProposal>;
  mergeEntity(vault: VaultId, id: string, input: { readonly into: string; readonly rationale: string }): Promise<EntityProposal>;
  splitEntity(vault: VaultId, id: string, input: { readonly baseVersion: number; readonly moved: readonly string[]; readonly aliases: readonly string[]; readonly fields: readonly EntityFieldInput[]; readonly rationale: string }): Promise<EntityProposal>;
}

/** A vault has at most 500 types and each page holds 200. */
const MAX_TYPE_PAGES = 5;
/** Suggestions are computed on read, so this stops at what a person could go through. */
const MAX_SAME_AS_PAGES = 5;

const isConflict = (failure: ApiFailure): boolean => failure.kind === "http" && (failure.code === "stale_version" || failure.status === 409);

interface EntityDeps {
  readonly edges: { readonly api: Api; readonly ids: IdSource };
  readonly watched: <T>(call: Promise<ApiResult<T>>) => Promise<ApiResult<T>>;
}

export const createEntityCases = ({ edges, watched }: EntityDeps): EntityCases => {
  const { api, ids } = edges;

  /** A write with no answer may still have landed, so a retry reuses its key and the server replays the first result. A key lives only until an answer arrives. */
  const keys = new Map<string, string>();
  const keyed = async <T>(what: string, call: (key: string) => Promise<ApiResult<T>>): Promise<ApiResult<T>> => {
    const key = keys.get(what) ?? ids.next();
    keys.set(what, key);
    const result = await watched(call(key));
    if (result.ok || result.failure.kind !== "unreachable") keys.delete(what);
    return result;
  };

  const outcome = (write: WikiWrite, entityId?: string): EntityProposal => {
    const id = entityId === undefined ? {} : { entityId };
    if (write.state === "committed") return { kind: "saved", changesetId: write.changesetId, ...id };
    if (write.state === "pending") return { kind: "pending", changesetId: write.changesetId, ...id };
    return { kind: "refused", reasons: write.refusals };
  };
  const failure = (f: ApiFailure): EntityProposal => (isConflict(f) ? { kind: "stale" } : { kind: "failed", failure: f });

  const pages = async <T>(max: number, read: (cursor: string | null) => Promise<ApiResult<Page<T>>>): Promise<ApiResult<{ readonly items: readonly T[]; readonly truncated: boolean }>> => {
    const items: T[] = [];
    let cursor: string | null = null;
    for (let page = 0; page < max; page += 1) {
      const result: ApiResult<Page<T>> = await watched(read(cursor));
      if (!result.ok) return failed(result.failure);
      items.push(...result.value.items);
      cursor = result.value.nextCursor;
      if (cursor === null) return ok({ items, truncated: false });
    }
    return ok({ items, truncated: true });
  };

  return {
    listEntityTypes: async (vault) => {
      const result = await pages(MAX_TYPE_PAGES, (cursor) => api.listEntityTypes(vault, cursor));
      return result.ok ? ok({ types: result.value.items, truncated: result.value.truncated }) : failed(result.failure);
    },
    listEntityTypeVersions: (vault, name) => watched(api.listEntityTypeVersions(vault, name)),
    previewEntityType: (vault, name, schema) => watched(api.previewEntityType(vault, name, schema)),
    submitEntityType: async (vault, { name, baseVersion, schema, rationale }) => {
      const result = await keyed(`type/${vault}/${name}/${String(baseVersion)}/${JSON.stringify(schema)}`, (idempotencyKey) => api.submitEntityType(vault, { name, baseVersion, schema, rationale, idempotencyKey }));
      return result.ok ? outcome(result.value) : failure(result.failure);
    },
    listEntities: (vault, type, filters, cursor) => watched(api.listEntities(vault, { type, filters: filters.map(entityFilterText), cursor })),
    getEntity: (vault, id) => watched(api.getEntity(vault, id)),
    getEntityRedirect: (vault, id) => watched(api.getEntityRedirect(vault, id)),
    resolveEntities: (vault, mention, type) => watched(api.resolveEntities(vault, mention, type)),
    listSameAs: async (vault, type) => {
      const result = await pages(MAX_SAME_AS_PAGES, (cursor) => api.listSameAs(vault, type, cursor));
      return result.ok ? ok(result.value.items) : failed(result.failure);
    },
    entityNeighborhood: async (vault, entity) => {
      const { claims, truncated } = graphClaims(entity);
      const asks = [{ id: entity.id, field: null as string | null }, ...claims];
      const results = await Promise.all(asks.map(async ({ id, field }) => ({ viaField: field, result: await watched(api.listNeighbors(vault, id)) })));
      const worked = results.flatMap(({ viaField, result }) => (result.ok ? [{ viaField, page: result.value }] : []));
      const first = results.find((r) => !r.result.ok);
      // Nothing came back at all, so there's no partial answer to show.
      if (worked.length === 0 && first !== undefined && !first.result.ok) return failed(first.result.failure);
      return ok({
        answers: worked.map(({ viaField, page }) => ({ viaField, items: page.items })),
        truncated: truncated || worked.some(({ page }) => page.nextCursor !== null),
        failures: results.length - worked.length,
      });
    },
    createEntity: async (vault, { type, aliases, fields, rationale }) => {
      const result = await keyed(`new/${vault}/${type}/${JSON.stringify([aliases, fields])}`, (idempotencyKey) => api.createEntity(vault, { type, aliases, fields, rationale, idempotencyKey }));
      return result.ok ? outcome(result.value.write, result.value.entityId) : failure(result.failure);
    },
    editEntity: async (vault, id, { baseVersion, aliases, fields, replacing, rationale }) => {
      const resolved: EntityFieldInput[] = [];
      for (const field of fields) {
        const claim = replacing[field.field];
        if (claim === undefined) {
          resolved.push(field);
          continue;
        }
        const version = await watched(api.readClaimVersion(vault, claim));
        if (!version.ok) return failure(version.failure);
        // Adding it beside the old one when asked to replace would say something else, so this stops.
        if (version.value === null) return { kind: "refused", reasons: [`The server didn't say which version the current value of ${field.field} is, so it can't be replaced.`] };
        resolved.push({ ...field, replaces: { assertionId: claim, version: version.value } });
      }
      const result = await keyed(`edit/${vault}/${id}/${String(baseVersion)}/${JSON.stringify([aliases, resolved])}`, (idempotencyKey) => api.editEntity(vault, id, { baseVersion, aliases, fields: resolved, rationale, idempotencyKey }));
      return result.ok ? outcome(result.value) : failure(result.failure);
    },
    mergeEntity: async (vault, id, { into, rationale }) => {
      const result = await keyed(`merge/${vault}/${id}/${into}`, (idempotencyKey) => api.mergeEntity(vault, id, { into, rationale, idempotencyKey }));
      return result.ok ? outcome(result.value) : failure(result.failure);
    },
    splitEntity: async (vault, id, { baseVersion, moved, aliases, fields, rationale }) => {
      const result = await keyed(`split/${vault}/${id}/${String(baseVersion)}/${JSON.stringify([moved, aliases, fields])}`, (idempotencyKey) => api.splitEntity(vault, id, { baseVersion, moved, aliases, fields, rationale, idempotencyKey }));
      return result.ok ? outcome(result.value.write, result.value.entityId) : failure(result.failure);
    },
  };
};
