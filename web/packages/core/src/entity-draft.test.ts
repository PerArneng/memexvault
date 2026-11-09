import type { EntityDetail, EntityFieldDetail, FieldClaim } from "@memex/types";
import { epochMs } from "@memex/types";
import { describe, expect, it } from "vitest";
import { approvalText, blankDraft, blankField, DRAFT_KINDS, draftFromSchema, draftProblems, entityGraph, graphClaims, MAX_GRAPH_CLAIMS, MAX_GRAPH_NODES, mergePreview, schemaFromDraft, splitPreview } from "./entity-draft";
import { schemaView } from "./entity-schema";

const claim = (id: string, value: unknown): FieldClaim => ({ assertionId: id, value, epistemic: "asserted", machine: false, creator: "pat", changesetId: "cs", evidence: [] });
const field = (name: string, value: unknown, conflicts: readonly unknown[] = []): EntityFieldDetail => ({ field: name, value, why: "the only claim", current: claim(`${name}-c`, value), conflicts: conflicts.map((c, i) => claim(`${name}-x${String(i)}`, c)), agreeing: [] });
const entity = (id: string, fields: readonly EntityFieldDetail[], aliases: readonly string[] = [], typeName = "T"): EntityDetail => ({ id, typeName, typeVersion: 1, version: 1, aliases, mergedFrom: [], redirectedFrom: null, changesetId: "cs", updatedAt: epochMs(0), fields });

describe("type drafts", () => {
  it("starts a new type closed, with a named identity", () => {
    const schema = schemaFromDraft(blankDraft("Thing", "k"));
    expect(schema).toMatchObject({ type: "object", additionalProperties: false, required: ["name"] });
    expect(schemaView(schema).fields[0]).toMatchObject({ name: "name", identity: true, filterable: true, searchable: true, required: true });
  });

  it("writes each kind the form offers so the schema reader reads it back", () => {
    const expected = { text: "text", number: "number", integer: "number", boolean: "boolean", date: "time", "date-time": "time", "text-list": "text", object: "other" } as const;
    for (const kind of DRAFT_KINDS) {
      const draft = { name: "T", outer: { type: "object", additionalProperties: false }, fields: [{ ...blankField("k"), name: "f", kind }] };
      const back = schemaView(schemaFromDraft(draft)).fields[0];
      expect(back?.kind).toBe(expected[kind]);
      expect(draftFromSchema("T", schemaFromDraft(draft)).fields[0]?.kind).toBe(kind);
    }
  });

  it("round-trips a schema with rules the form doesn't show, so opening it changes nothing", () => {
    const schema = {
      type: "object",
      additionalProperties: false,
      title: "Product",
      required: ["name"],
      properties: {
        name: { type: "string", identity: true, searchable: true, filterable: true, maxLength: 80, description: "Its name" },
        price: { type: "number", filterable: true, minimum: 0 },
        grade: { type: "string", enum: ["a", "b"] },
        specs: { type: "object", properties: { w: { type: "number" } } },
      },
    };
    expect(schemaFromDraft(draftFromSchema("Product", schema))).toEqual(schema);
  });

  it("drops a field's old rules when its kind changes, and an annotation when unticked", () => {
    const draft = draftFromSchema("T", { type: "object", additionalProperties: false, properties: { a: { type: "string", maxLength: 3, filterable: true } } });
    const [a] = draft.fields;
    if (a === undefined) throw new Error("no field");
    const changed = schemaFromDraft({ ...draft, fields: [{ ...a, kind: "number", filterable: false }] });
    expect(changed.properties).toEqual({ a: { type: "number" } });
  });

  it("reads a schema that isn't one without throwing", () => {
    for (const bad of [null, 7, "x", [], { properties: 1 }, { properties: { a: 1 } }]) {
      expect(draftFromSchema("T", bad).fields).toEqual([]);
    }
  });

  it("finds what the server would refuse", () => {
    const fine = blankDraft("Thing", "k");
    expect(draftProblems(fine)).toEqual([]);
    const f = fine.fields[0];
    if (f === undefined) throw new Error("no field");
    expect(draftProblems({ ...fine, name: "1bad" })[0]).toContain("type's name");
    expect(draftProblems({ ...fine, name: "x".repeat(65) })[0]).toContain("type's name");
    expect(draftProblems({ ...fine, fields: [] })[0]).toContain("at least one");
    expect(draftProblems({ ...fine, fields: [{ ...f, name: "a b" }] })[0]).toContain("isn't allowed");
    expect(draftProblems({ ...fine, fields: [f, { ...f, key: "other" }] })[0]).toContain("Two fields");
    expect(draftProblems({ ...fine, fields: [{ ...f, kind: "object", identity: false }] })[0]).toContain("JSON");
    expect(draftProblems({ ...fine, fields: [{ ...f, kind: "number", filterable: true, searchable: true }] })[0]).toContain("must hold text");
  });
});

describe("approvalText", () => {
  it("says what each decision needs", () => {
    expect(approvalText("auto_commit", null).title).toBe("No approval needed.");
    expect(approvalText("review", null).title).toBe("A person has to approve this.");
    expect(approvalText("reject", null).tone).toBe("danger");
    expect(approvalText("auto_commit", "a bad schema")).toMatchObject({ tone: "danger", detail: "a bad schema" });
  });
  it("treats a word it doesn't know as needing a person", () => {
    expect(approvalText("something_new", null).title).toBe("A person has to approve this.");
  });
});

describe("mergePreview", () => {
  it("says what happens to each field, with the aliases and the redirect", () => {
    const source = entity("s", [field("a", "same"), field("b", "from source"), field("c", "one")], ["x", "y"]);
    const target = entity("t", [field("a", "same"), field("c", "two"), field("d", "stays")], ["y", "z"]);
    const preview = mergePreview(source, target);
    expect(preview.rows.map((r) => [r.field, r.outcome])).toEqual([["a", "same"], ["b", "from_source"], ["c", "conflict"], ["d", "stays"]]);
    expect(preview.aliases).toEqual(["y", "z", "x"]);
    expect(preview.redirect).toEqual({ from: "s", to: "t" });
    expect(preview.sameType).toBe(true);
  });
  it("notices different types", () => {
    expect(mergePreview(entity("s", [], [], "A"), entity("t", [], [], "B")).sameType).toBe(false);
  });
});

describe("splitPreview", () => {
  it("divides claims and aliases between the two", () => {
    const e = entity("e", [field("a", "1", ["2"]), field("b", "3")], ["x", "y"]);
    const preview = splitPreview(e, new Set(["a-x0"]), new Set(["y"]));
    expect(preview.moved).toEqual([{ field: "a", value: "2" }]);
    expect(preview.staying).toEqual([{ field: "a", value: "1" }, { field: "b", value: "3" }]);
    expect(preview.movedAliases).toEqual(["y"]);
    expect(preview.stayingAliases).toEqual(["x"]);
  });
});

describe("the graph", () => {
  const link = (edge: string, neighbor: string | null, title: string | null = null) => ({ edgeId: edge, kind: "claim_ref", direction: "in", from: neighbor ?? "x", to: "y", neighbor, title });

  it("merges links to the same object, and leaves out the entity and its own claims", () => {
    const graph = entityGraph("e", [
      { viaField: null, items: [link("1", "e")] },
      { viaField: "a", items: [link("2", "page", "Page"), link("3", "a-c")] },
      { viaField: "b", items: [link("4", "page", "Page"), link("4", "page", "Page")] },
    ], new Set(["a-c"]));
    expect(graph.nodes.map((n) => n.objectId)).toEqual(["page"]);
    expect(graph.nodes[0]?.links.map((l) => [l.edgeId, l.viaField])).toEqual([["2", "a"], ["4", "b"]]);
  });

  it("keeps a link to nothing as a node of its own, and every node inside the panel", () => {
    const graph = entityGraph("e", [{ viaField: null, items: [link("1", null), link("2", "x"), link("3", "y")] }], new Set());
    expect(graph.nodes).toHaveLength(3);
    for (const n of graph.nodes) {
      expect(n.x).toBeGreaterThan(0);
      expect(n.x).toBeLessThan(100);
      expect(n.y).toBeGreaterThan(0);
      expect(n.y).toBeLessThan(100);
    }
  });

  it("stops at its limit and says so", () => {
    const items = Array.from({ length: MAX_GRAPH_NODES + 5 }, (_, i) => link(String(i), `n${String(i)}`));
    const graph = entityGraph("e", [{ viaField: null, items }], new Set());
    expect(graph.nodes).toHaveLength(MAX_GRAPH_NODES);
    expect(graph.truncated).toBe(true);
  });

  it("asks about a bounded number of claims", () => {
    const fields = Array.from({ length: MAX_GRAPH_CLAIMS + 1 }, (_, i) => field(`f${String(i)}`, i));
    const { claims, truncated } = graphClaims(entity("e", fields));
    expect(claims).toHaveLength(MAX_GRAPH_CLAIMS);
    expect(truncated).toBe(true);
    expect(graphClaims(entity("e", [field("a", 1, [2])])).claims.map((c) => c.id)).toEqual(["a-c", "a-x0"]);
  });
});
