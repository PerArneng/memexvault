import { describe, expect, it } from "vitest";
import { attributeOf, entityLabel, filterProblem, MAX_DEFAULT_COLUMNS, MAX_SCHEMA_FIELDS, parseFieldValue, schemaView, sortLoadedRows, valueText } from "./entity-schema";
import type { EntityFilterDef, SchemaField } from "./entity-schema";

const schemaOf = (properties: unknown, required?: unknown) => ({ type: "object", additionalProperties: false, properties, ...(required === undefined ? {} : { required }) });
const one = (def: unknown) => {
  const view = schemaView(schemaOf({ f: def }));
  const [field] = view.fields;
  if (field === undefined) throw new Error("no field");
  return { view, field, column: view.columns[0], filter: view.filters[0] };
};

describe("schemaView: every field type", () => {
  const cases: readonly { readonly name: string; readonly def: Readonly<Record<string, unknown>>; readonly kind: string; readonly numeric: boolean; readonly ops: readonly string[]; readonly input: string; readonly isArray?: boolean; readonly integer?: boolean }[] = [
    { name: "string", def: { type: "string" }, kind: "text", numeric: false, ops: ["eq"], input: "text" },
    { name: "string with enum", def: { type: "string", enum: ["a", "b"] }, kind: "text", numeric: false, ops: ["eq"], input: "choice" },
    { name: "string with another format", def: { type: "string", format: "email" }, kind: "text", numeric: false, ops: ["eq"], input: "text" },
    { name: "number", def: { type: "number" }, kind: "number", numeric: true, ops: ["eq", "lt", "lte", "gt", "gte"], input: "number" },
    { name: "integer", def: { type: "integer" }, kind: "number", numeric: true, ops: ["eq", "lt", "lte", "gt", "gte"], input: "number", integer: true },
    { name: "boolean", def: { type: "boolean" }, kind: "boolean", numeric: false, ops: ["eq"], input: "boolean" },
    { name: "date", def: { type: "string", format: "date" }, kind: "time", numeric: false, ops: ["eq", "lt", "lte", "gt", "gte"], input: "date" },
    { name: "date-time", def: { type: "string", format: "date-time" }, kind: "time", numeric: false, ops: ["eq", "lt", "lte", "gt", "gte"], input: "date-time" },
    { name: "nullable string", def: { type: ["null", "string"] }, kind: "text", numeric: false, ops: ["eq"], input: "text" },
    { name: "array of strings", def: { type: "array", items: { type: "string" } }, kind: "text", numeric: false, ops: ["eq"], input: "text", isArray: true },
    { name: "array of numbers", def: { type: "array", items: { type: "number" } }, kind: "number", numeric: false, ops: ["eq", "lt", "lte", "gt", "gte"], input: "number", isArray: true },
  ];
  for (const c of cases) {
    it(`maps ${c.name} to a column and a filter`, () => {
      const { field, column, filter, view } = one({ ...c.def, filterable: true });
      expect(view.problems).toEqual([]);
      expect(field.kind).toBe(c.kind);
      expect(field.isArray).toBe(c.isArray ?? false);
      expect(field.integer).toBe(c.integer ?? false);
      expect(column?.numeric).toBe(c.numeric);
      expect(filter?.ops).toEqual(c.ops);
      expect(filter?.input).toBe(c.input);
    });
  }

  it("maps what has no one comparable value to a column with no filter, whatever its annotation says", () => {
    for (const def of [{ type: "object" }, { type: "array", items: { type: "object" } }, { type: "array" }, {}, { enum: ["a"] }, { type: 7 }, { $ref: "#/x" }]) {
      const { field, column, view } = one({ ...def, filterable: true });
      expect(field.kind).toBe("other");
      expect(column?.kind).toBe("other");
      expect(view.filters).toEqual([]);
    }
  });

  it("reads the annotations, required and description", () => {
    const view = schemaView(schemaOf({ a: { type: "string", identity: true, searchable: true, description: "An a" }, b: { type: "number", filterable: "yes" } }, ["a", 7]));
    const [a, b] = view.fields;
    expect(a).toMatchObject({ name: "a", identity: true, searchable: true, filterable: false, required: true, description: "An a" });
    expect(b).toMatchObject({ name: "b", identity: false, filterable: false, required: false, description: null });
    expect(view.filters).toEqual([]);
  });

  it("puts identity first, then filterable, and shows a limited number by default", () => {
    const properties = Object.fromEntries(Array.from({ length: 10 }, (_, i) => [`f${String(i)}`, { type: "string" }]));
    const view = schemaView(schemaOf({ ...properties, zid: { type: "string", identity: true }, yf: { type: "string", filterable: true } }));
    expect(view.columns.slice(0, 2).map((c) => c.id)).toEqual(["zid", "yf"]);
    expect(view.columns.filter((c) => c.visibleByDefault)).toHaveLength(MAX_DEFAULT_COLUMNS);
    expect(view.columns.at(-1)?.visibleByDefault).toBe(false);
  });
});

describe("schemaView: malformed", () => {
  it("says so rather than throwing, for anything that isn't a schema", () => {
    for (const bad of [null, undefined, 7, "text", true, [], [{}], { type: "object" }, { properties: null }, { properties: [] }, { properties: "x" }]) {
      const view = schemaView(bad);
      expect(view.fields).toEqual([]);
      expect(view.columns).toEqual([]);
      expect(view.filters).toEqual([]);
      expect(view.problems).toHaveLength(1);
    }
  });

  it("leaves out a field that isn't described by an object, and keeps the rest", () => {
    const view = schemaView(schemaOf({ bad: "text", worse: null, list: [], good: { type: "string" } }));
    expect(view.fields.map((f) => f.name)).toEqual(["good"]);
    expect(view.problems).toHaveLength(3);
  });

  it("copes with a required that isn't a list, and an enum of the wrong things", () => {
    const view = schemaView(schemaOf({ a: { type: "string", enum: [1, null, "ok"] } }, "a"));
    expect(view.fields[0]?.required).toBe(false);
    expect(view.fields[0]?.enumValues).toEqual(["ok"]);
  });

  it("treats field names that mean something in JavaScript as plain fields", () => {
    const properties = JSON.parse('{"__proto__": {"type": "string"}, "constructor": {"type": "number"}, "toString": {"type": "boolean"}}') as unknown;
    const view = schemaView(schemaOf(properties));
    expect(view.fields.map((f) => f.name)).toEqual(["__proto__", "constructor", "toString"]);
    expect(view.fields.map((f) => f.kind)).toEqual(["text", "number", "boolean"]);
    const attributes = JSON.parse('{"__proto__": "value"}') as Record<string, unknown>;
    expect(attributeOf(attributes, "__proto__")).toBe("value");
    expect(attributeOf({}, "constructor")).toBeUndefined();
    expect(attributeOf({}, "toString")).toBeUndefined();
  });

  it("keeps markup in a field's name and description as data", () => {
    const view = schemaView(schemaOf({ "<img src=x>": { type: "string", description: "<script>1</script>" } }));
    expect(view.fields[0]?.name).toBe("<img src=x>");
    expect(view.fields[0]?.description).toBe("<script>1</script>");
    expect(view.columns[0]?.label).toBe("<img src=x>");
  });
});

describe("schemaView: very large", () => {
  it("holds at the server's limit exactly", () => {
    const properties = Object.fromEntries(Array.from({ length: MAX_SCHEMA_FIELDS }, (_, i) => [`f${String(i).padStart(4, "0")}`, { type: "string", filterable: true }]));
    const view = schemaView(schemaOf(properties));
    expect(view.fields).toHaveLength(MAX_SCHEMA_FIELDS);
    expect(view.truncated).toBe(false);
    expect(view.problems).toEqual([]);
  });

  it("cuts 100,000 fields to the limit and says so", () => {
    const properties = Object.fromEntries(Array.from({ length: 100_000 }, (_, i) => [`f${String(i).padStart(6, "0")}`, { type: "string", filterable: true }]));
    const view = schemaView(schemaOf(properties));
    expect(view.fields).toHaveLength(MAX_SCHEMA_FIELDS);
    expect(view.columns).toHaveLength(MAX_SCHEMA_FIELDS);
    expect(view.filters).toHaveLength(MAX_SCHEMA_FIELDS);
    expect(view.truncated).toBe(true);
    expect(view.problems.join(" ")).toContain("100000 fields");
  });

  it("caps its reports, a long name and a long enum", () => {
    const view = schemaView(schemaOf({
      ...Object.fromEntries(Array.from({ length: 150 }, (_, i) => [`bad${String(i)}`, 1])),
      ["n".repeat(100_000)]: { type: "string" },
      big: { type: "string", enum: Array.from({ length: 100_000 }, (_, i) => String(i)) },
    }));
    expect(view.problems.length).toBeLessThanOrEqual(21);
    expect(view.problems.every((p) => p.length < 200)).toBe(true);
    expect(view.fields.map((f) => f.name)).toEqual(["big"]);
    expect(view.fields[0]?.enumValues).toHaveLength(200);
  });

  it("survives a deeply nested description of one field", () => {
    let deep: Record<string, unknown> = { type: "string" };
    for (let i = 0; i < 10_000; i += 1) deep = { type: "object", properties: { x: deep } };
    const view = schemaView(schemaOf({ deep }));
    expect(view.fields[0]?.kind).toBe("other");
  });
});

describe("valueText", () => {
  it("writes each kind of value as text", () => {
    expect(valueText("a")).toBe("a");
    expect(valueText(1.5)).toBe("1.5");
    expect(valueText(false)).toBe("false");
    expect(valueText(null)).toBe("");
    expect(valueText(undefined)).toBe("");
    expect(valueText(["a", 2])).toBe("a, 2");
    expect(valueText({ a: 1 })).toBe('{"a":1}');
    expect(valueText([{ a: 1 }])).toBe('[{"a":1}]');
  });
  it("leaves markup as it is, for a text node, and cuts a huge value", () => {
    expect(valueText("<img src=x onerror=alert(1)>")).toBe("<img src=x onerror=alert(1)>");
    expect(valueText("x".repeat(100_000))).toHaveLength(501);
  });
});

describe("sortLoadedRows", () => {
  const rows = [{ v: 2 }, { v: null }, { v: 10 }, { v: 1 }];
  it("sorts numbers as numbers, both ways, with empty values last", () => {
    expect(sortLoadedRows(rows, (r) => r.v, "number", "asc").map((r) => r.v)).toEqual([1, 2, 10, null]);
    expect(sortLoadedRows(rows, (r) => r.v, "number", "desc").map((r) => r.v)).toEqual([10, 2, 1, null]);
  });
  it("sorts text ignoring case, and keeps the order of equal rows", () => {
    const text = [{ v: "b", n: 1 }, { v: "A", n: 2 }, { v: "a", n: 3 }];
    expect(sortLoadedRows(text, (r) => r.v, "text", "asc").map((r) => r.n)).toEqual([2, 3, 1]);
  });
  it("copes with a value of the wrong type in a numeric column", () => {
    const mixed = [{ v: 2 }, { v: "x" }, { v: 1 }];
    expect(() => sortLoadedRows(mixed, (r) => r.v, "number", "asc")).not.toThrow();
  });
  it("doesn't change what it's given", () => {
    const before = [...rows];
    sortLoadedRows(rows, (r) => r.v, "number", "asc");
    expect(rows).toEqual(before);
  });
});

describe("filterProblem", () => {
  const def = (kind: EntityFilterDef["kind"], ops: EntityFilterDef["ops"]): EntityFilterDef => ({ field: "f", label: "f", kind, ops, input: "text", options: [] });
  it("accepts what the server accepts", () => {
    expect(filterProblem(def("number", ["eq", "lt"]), "lt", "1.5")).toBeNull();
    expect(filterProblem(def("time", ["eq"]), "eq", "2026-01-31")).toBeNull();
    expect(filterProblem(def("time", ["eq"]), "eq", "2026-01-31T10:00:00Z")).toBeNull();
    expect(filterProblem(def("boolean", ["eq"]), "eq", "true")).toBeNull();
    expect(filterProblem(def("text", ["eq"]), "eq", "anything: at all")).toBeNull();
  });
  it("refuses what it would refuse", () => {
    expect(filterProblem(def("number", ["eq"]), "eq", "many")).toContain("numbers");
    expect(filterProblem(def("number", ["eq"]), "eq", "Infinity")).toContain("numbers");
    expect(filterProblem(def("time", ["eq"]), "eq", "soon")).toContain("dates");
    expect(filterProblem(def("boolean", ["eq"]), "eq", "yes")).toContain("true or false");
    expect(filterProblem(def("text", ["eq"]), "lt", "a")).toContain("is");
    expect(filterProblem(def("text", ["eq"]), "eq", "  ")).toContain("Give a value");
  });
});

describe("parseFieldValue", () => {
  const field = (over: Partial<SchemaField>): SchemaField => ({ name: "f", kind: "text", isArray: false, integer: false, format: null, searchable: false, filterable: false, identity: false, required: false, enumValues: null, description: null, ...over });
  it("reads each kind", () => {
    expect(parseFieldValue(field({}), " a ")).toEqual({ ok: true, value: " a " });
    expect(parseFieldValue(field({ kind: "number" }), "1.5")).toEqual({ ok: true, value: 1.5 });
    expect(parseFieldValue(field({ kind: "boolean" }), "false")).toEqual({ ok: true, value: false });
    expect(parseFieldValue(field({ kind: "time" }), "2026-01-31")).toEqual({ ok: true, value: "2026-01-31" });
    expect(parseFieldValue(field({ isArray: true }), "a, b,,")).toEqual({ ok: true, value: ["a", "b"] });
    expect(parseFieldValue(field({ kind: "number", isArray: true }), "1,2")).toEqual({ ok: true, value: [1, 2] });
    expect(parseFieldValue(field({ kind: "other" }), '{"a":1}')).toEqual({ ok: true, value: { a: 1 } });
  });
  it("says what's wrong", () => {
    for (const bad of [field({ kind: "number" }), field({ kind: "boolean" }), field({ kind: "time" }), field({ kind: "other" }), field({ kind: "number", isArray: true })]) {
      expect(parseFieldValue(bad, "{nope").ok).toBe(false);
    }
    expect(parseFieldValue(field({ kind: "number", integer: true }), "1.5").ok).toBe(false);
  });
});

describe("entityLabel", () => {
  const fields = schemaView(schemaOf({ a: { type: "string" }, name: { type: "string", identity: true } })).fields;
  it("prefers an identity field, then an alias", () => {
    expect(entityLabel({ name: "N" }, ["alias"], fields)).toBe("N");
    expect(entityLabel({ name: "" }, ["alias"], fields)).toBe("alias");
    expect(entityLabel({}, [], fields)).toBeNull();
  });
});
