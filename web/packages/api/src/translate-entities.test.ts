import { describe, expect, it } from "vitest";
import type { components } from "./generated/schema";
import { claimVersionFromWire, entityDetailFromWire, entityPageFromWire, entityResolutionFromWire, entityTypePreviewFromWire, neighborPageFromWire, sameAsPageFromWire } from "./translate-entities";

type Schemas = components["schemas"];

const entity: Schemas["EntityDto"] = { id: "e", type: "T", type_version: 1, version: 2, aliases: ["a"], attributes: { price: 1 } as never, conflicted_fields: ["price"], merged_from: [], updated_at_ms: 5 };

describe("entity translation", () => {
  it("reads a page of entities, with no next cursor as null", () => {
    expect(entityPageFromWire({ items: [entity] })).toEqual({ nextCursor: null, items: [{ id: "e", typeName: "T", typeVersion: 1, version: 2, aliases: ["a"], attributes: { price: 1 }, conflictedFields: ["price"], mergedFrom: [], updatedAt: 5 }] });
  });

  it("keeps attributes that aren't an object from breaking a table", () => {
    const page = entityPageFromWire({ items: [{ ...entity, attributes: 7 as never }] });
    expect(page.items[0]?.attributes).toEqual({});
  });

  it("reads a detail with its evidence, conflicts and missing facts", () => {
    const claim: Schemas["FieldClaimDto"] = { assertion_id: "c", value: 1 as never, evidence: [{ evidence_id: "ev", quote: "q", stance: "supports", segment_id: "s", anchor_broken: false }] };
    const detail = entityDetailFromWire({ id: "e", type: "T", type_version: 1, version: 2, aliases: [], merged_from: ["old"], changeset_id: "cs", updated_at_ms: 5, fields: [{ field: "f", value: 1 as never, why: "because", current: claim, conflicts: [{ ...claim, assertion_id: "d", value: 2 as never }], agreeing: [] }] });
    expect(detail.redirectedFrom).toBeNull();
    expect(detail.fields[0]?.why).toBe("because");
    expect(detail.fields[0]?.conflicts[0]?.value).toBe(2);
    expect(detail.fields[0]?.current).toMatchObject({ epistemic: null, machine: null, creator: null, changesetId: null });
    expect(detail.fields[0]?.current.evidence[0]).toMatchObject({ evidenceId: "ev", sourceId: null, pageFrom: null, anchorBroken: false });
  });

  it("reads the rest", () => {
    expect(entityTypePreviewFromWire({ change: "breaking", reasons: ["r"], next_version: 2, decision: "review" })).toEqual({ problem: null, change: "breaking", reasons: ["r"], currentVersion: null, nextVersion: 2, decision: "review" });
    expect(entityResolutionFromWire({ candidates: [{ entity, exact: false, score: 1 }], degraded: ["lexical"] }).candidates[0]?.matchedOn).toBeNull();
    expect(sameAsPageFromWire({ items: [{ field: "f", value: "v", entity_ids: ["a", "b"] }] }).items[0]?.entityIds).toEqual(["a", "b"]);
    expect(neighborPageFromWire({ items: [{ edge_id: "x", kind: "k", direction: "in", from: "a", to: "b" }] }).items[0]).toMatchObject({ neighbor: null, title: null });
  });

  it("finds a claim's version in its stored row, and says when it can't", () => {
    const lineage = (assertion: unknown) => ({ assertion, changeset_id: "cs", evidence: [] }) as unknown as Schemas["LineageDto"];
    expect(claimVersionFromWire(lineage({ header: { version: 4 } }))).toBe(4);
    expect(claimVersionFromWire(lineage({ header: {} }))).toBeNull();
    expect(claimVersionFromWire(lineage(null))).toBeNull();
  });
});
