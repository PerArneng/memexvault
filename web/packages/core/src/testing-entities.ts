import type { EntityDetail, EntitySummary, EntityType, Neighbor, Page, SameAsSuggestion, WikiWrite } from "@memex/types";
import { failed, ok } from "./api";
import type { Api, ApiFailure, ApiResult } from "./api";

export interface EntitiesSeed {
  readonly types?: readonly EntityType[];
  readonly entities?: readonly EntitySummary[];
  readonly details?: Readonly<Record<string, EntityDetail>>;
  /** Rows per list page, to exercise cursors. */
  readonly pageSize?: number;
  /** What a proposal about a type comes back as. */
  readonly typeWrite?: WikiWrite["state"];
  /** What a proposal about an entity comes back as. Merges and splits are always pending, as on the server. */
  readonly entityWrite?: WikiWrite["state"];
  readonly sameAs?: readonly SameAsSuggestion[];
  readonly neighbors?: Readonly<Record<string, readonly Neighbor[]>>;
  readonly claimVersions?: Readonly<Record<string, number>>;
}

type EntityApi = Pick<Api, "listEntityTypes" | "listEntityTypeVersions" | "previewEntityType" | "submitEntityType" | "listEntities" | "getEntity" | "getEntityRedirect" | "createEntity" | "editEntity" | "mergeEntity" | "splitEntity" | "resolveEntities" | "listSameAs" | "listNeighbors" | "readClaimVersion">;

const NOT_FOUND: ApiFailure = { kind: "http", status: 404, code: "not_found", message: "not_found", requestId: "req-1" };

const pageFrom = <T>(all: readonly T[], cursor: string | null, size: number): Page<T> => {
  const start = cursor === null ? 0 : Number(cursor);
  const end = start + size;
  return { items: all.slice(start, end), nextCursor: end < all.length ? String(end) : null };
};

/** The entity routes on a fake server. Keeps `testing.ts` itself small, as several screens add to it. */
export const fakeEntityApi = (seed: EntitiesSeed, answer: <T>(name: string, value: () => ApiResult<T>) => Promise<ApiResult<T>>): EntityApi => {
  const write = (state: WikiWrite["state"], id: string): WikiWrite => ({ changesetId: state === "rejected" ? null : id, state, replayed: false, refusals: state === "rejected" ? ["refused by the fake"] : [] });
  const size = seed.pageSize ?? 200;
  const rows = seed.entities ?? [];
  return {
    listEntityTypes: (_vault, cursor) => answer("listEntityTypes", () => ok(pageFrom(seed.types ?? [], cursor, size))),
    listEntityTypeVersions: (_vault, name) => answer("listEntityTypeVersions", () => {
      const versions = (seed.types ?? []).filter((t) => t.name === name);
      return versions.length === 0 ? failed(NOT_FOUND) : ok(versions);
    }),
    previewEntityType: () => answer("previewEntityType", () => ok({ problem: null, change: "additive", reasons: [], currentVersion: 1, nextVersion: 2, decision: "auto_commit" })),
    submitEntityType: (_vault, input) => answer("submitEntityType:" + input.idempotencyKey, () => ok(write(seed.typeWrite ?? "committed", `cs-type-${input.name}`))),
    listEntities: (_vault, { type, filters, cursor }) => answer(`listEntities:${type}:${filters.join("&")}`, () => ok(pageFrom(rows.filter((r) => r.typeName === type), cursor, size))),
    getEntity: (_vault, id) => answer("getEntity", () => {
      const found = seed.details?.[id];
      return found === undefined ? failed(NOT_FOUND) : ok(found);
    }),
    getEntityRedirect: (_vault, id) => answer("getEntityRedirect", () => ok({ id, currentId: id, chain: [] })),
    createEntity: (_vault, input) => answer("createEntity:" + input.idempotencyKey, () => ok({ entityId: "entity-new", write: write(seed.entityWrite ?? "committed", "cs-new") })),
    editEntity: (_vault, id, input) => answer(`editEntity:${JSON.stringify(input.fields.map((f) => f.replaces))}`, () => (seed.details?.[id] === undefined ? failed(NOT_FOUND) : ok(write(seed.entityWrite ?? "committed", "cs-edit")))),
    mergeEntity: (_vault, id) => answer("mergeEntity", () => ok(write("pending", `cs-merge-${id}`))),
    splitEntity: (_vault, id) => answer("splitEntity", () => ok({ entityId: "entity-split", write: write("pending", `cs-split-${id}`) })),
    resolveEntities: (_vault, mention) => answer("resolveEntities", () => ok({ candidates: rows.filter((r) => r.aliases.includes(mention)).map((entity) => ({ entity, matchedOn: "$alias", exact: true, score: 1 })), degraded: [] })),
    listSameAs: (_vault, _type, cursor) => answer("listSameAs", () => ok(pageFrom(seed.sameAs ?? [], cursor, size))),
    listNeighbors: (_vault, id) => answer(`listNeighbors:${id}`, () => ok({ items: seed.neighbors?.[id] ?? [], nextCursor: null })),
    readClaimVersion: (_vault, id) => answer("readClaimVersion", () => ok(seed.claimVersions?.[id] ?? null)),
  };
};
