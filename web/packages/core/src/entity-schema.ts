/**
 * From a type's JSON Schema to what a screen needs: columns, typed filters and form fields. A new type works with no frontend change because this is
 * the only place that reads a schema, and it mirrors `fields_of` in `memex-core`'s `entity.rs`. A schema is vault content, so nothing here trusts its
 * shape: anything malformed becomes a problem to show, never an exception.
 */

export type FieldKind = "text" | "number" | "boolean" | "time" | "other";

export interface SchemaField {
  readonly name: string;
  readonly kind: FieldKind;
  readonly isArray: boolean;
  /** `integer` rather than `number`, so a form can refuse 1.5. */
  readonly integer: boolean;
  /** `date`, `date-time`, or another format as the schema says it. */
  readonly format: string | null;
  readonly searchable: boolean;
  readonly filterable: boolean;
  readonly identity: boolean;
  readonly required: boolean;
  readonly enumValues: readonly string[] | null;
  readonly description: string | null;
}

export type FilterOp = "eq" | "lt" | "lte" | "gt" | "gte";

export const FILTER_OP_LABELS: Readonly<Record<FilterOp, string>> = { eq: "is", lt: "below", lte: "at most", gt: "above", gte: "at least" };

export interface EntityColumn {
  readonly id: string;
  readonly label: string;
  readonly kind: FieldKind;
  readonly numeric: boolean;
  readonly width: string;
  /** Shown until someone chooses columns. */
  readonly visibleByDefault: boolean;
}

export type FilterInput = "text" | "number" | "date" | "date-time" | "boolean" | "choice";

export interface EntityFilterDef {
  readonly field: string;
  readonly label: string;
  readonly kind: FieldKind;
  readonly ops: readonly FilterOp[];
  readonly input: FilterInput;
  readonly options: readonly string[];
}

export interface SchemaView {
  readonly fields: readonly SchemaField[];
  readonly columns: readonly EntityColumn[];
  readonly filters: readonly EntityFilterDef[];
  /** What couldn't be read, in words a person can act on. */
  readonly problems: readonly string[];
  /** There were more fields than a table can hold, so only the first are here. */
  readonly truncated: boolean;
}

/** The server's limit per type. A stored schema can't pass it, so more means this isn't one, and the screen must not hang on it. */
export const MAX_SCHEMA_FIELDS = 200;
export const MAX_DEFAULT_COLUMNS = 6;
const MAX_ENUM_OPTIONS = 200;
const MAX_NAME_CHARS = 64;
const MAX_REPORTED_PROBLEMS = 20;
const MAX_CELL_CHARS = 500;

const isRecord = (value: unknown): value is Readonly<Record<string, unknown>> => typeof value === "object" && value !== null && !Array.isArray(value);
const flag = (def: Readonly<Record<string, unknown>>, name: string): boolean => def[name] === true;

const declaredType = (def: Readonly<Record<string, unknown>>): string | null => {
  const t = def.type;
  if (typeof t === "string") return t;
  if (Array.isArray(t)) {
    const named: unknown = t.find((x: unknown) => typeof x === "string" && x !== "null");
    return typeof named === "string" ? named : null;
  }
  return null;
};

const scalarKind = (def: Readonly<Record<string, unknown>>): FieldKind => {
  switch (declaredType(def)) {
    case "string":
      return def.format === "date" || def.format === "date-time" ? "time" : "text";
    case "number":
    case "integer":
      return "number";
    case "boolean":
      return "boolean";
    default:
      return "other";
  }
};

const fieldOf = (name: string, def: Readonly<Record<string, unknown>>, required: boolean): SchemaField => {
  const items = declaredType(def) === "array" && isRecord(def.items) ? def.items : null;
  const scalar = items ?? def;
  const enumRaw = Array.isArray(scalar.enum) ? scalar.enum : null;
  return {
    name,
    kind: scalarKind(scalar),
    isArray: items !== null,
    integer: declaredType(scalar) === "integer",
    format: typeof scalar.format === "string" ? scalar.format : null,
    searchable: flag(def, "searchable"),
    filterable: flag(def, "filterable"),
    identity: flag(def, "identity"),
    required,
    enumValues: enumRaw === null ? null : enumRaw.filter((v: unknown): v is string => typeof v === "string").slice(0, MAX_ENUM_OPTIONS),
    description: typeof def.description === "string" ? def.description : null,
  };
};

const widthOf = (field: SchemaField): string => {
  if (field.isArray) return "minmax(8rem, 1.5fr)";
  switch (field.kind) {
    case "number":
      return "7rem";
    case "boolean":
      return "5rem";
    case "time":
      return "11rem";
    default:
      return "minmax(8rem, 1.5fr)";
  }
};

const inputOf = (field: SchemaField): FilterInput => {
  if (field.kind === "boolean") return "boolean";
  if (field.kind === "number") return "number";
  if (field.kind === "time") return field.format === "date" ? "date" : "date-time";
  return field.enumValues !== null && field.enumValues.length > 0 ? "choice" : "text";
};

/** What the server's `parse_filter` accepts for each kind: text and booleans only compare equal. */
const opsOf = (kind: FieldKind): readonly FilterOp[] => (kind === "number" || kind === "time" ? ["eq", "lt", "lte", "gt", "gte"] : ["eq"]);

const columnOrder = (a: SchemaField, b: SchemaField): number => Number(b.identity) - Number(a.identity) || Number(b.filterable) - Number(a.filterable) || (a.name < b.name ? -1 : 1);

/** Everything a screen reads from a schema. Pure, total, and bounded however large the schema is. */
export const schemaView = (schema: unknown): SchemaView => {
  const empty = (problem: string): SchemaView => ({ fields: [], columns: [], filters: [], problems: [problem], truncated: false });
  if (!isRecord(schema)) return empty("This type's schema isn't an object, so it has no fields to show.");
  if (!isRecord(schema.properties)) return empty("This type's schema has no \"properties\", so it has no fields to show.");
  const required = new Set(Array.isArray(schema.required) ? schema.required.filter((r: unknown): r is string => typeof r === "string") : []);
  const names = Object.keys(schema.properties).sort();
  const truncated = names.length > MAX_SCHEMA_FIELDS;
  const problems: string[] = [];
  const skipped = (message: string) => {
    if (problems.length < MAX_REPORTED_PROBLEMS) problems.push(message);
  };
  const fields: SchemaField[] = [];
  for (const name of names.slice(0, MAX_SCHEMA_FIELDS)) {
    const def = schema.properties[name];
    const shown = name.length > MAX_NAME_CHARS ? `${name.slice(0, MAX_NAME_CHARS)}…` : name;
    if (name.length > MAX_NAME_CHARS) skipped(`The field name ${shown} is too long to be a field, so it's left out.`);
    else if (!isRecord(def)) skipped(`The field ${shown} isn't described by an object, so it's left out.`);
    else fields.push(fieldOf(name, def, required.has(name)));
  }
  if (truncated) problems.push(`This schema has ${String(names.length)} fields, so only the first ${String(MAX_SCHEMA_FIELDS)} are shown.`);
  const ordered = [...fields].sort(columnOrder);
  const columns: EntityColumn[] = ordered.map((f, i) => ({
    id: f.name,
    label: f.name,
    kind: f.kind,
    numeric: f.kind === "number" && !f.isArray,
    width: widthOf(f),
    visibleByDefault: i < MAX_DEFAULT_COLUMNS,
  }));
  const filters: EntityFilterDef[] = ordered
    // The server says a field with no one comparable value can't be compared, whatever its annotation.
    .filter((f) => f.filterable && f.kind !== "other")
    .map((f) => ({ field: f.name, label: f.name, kind: f.kind, ops: opsOf(f.kind), input: inputOf(f), options: f.enumValues ?? [] }));
  return { fields, columns, filters, problems, truncated };
};

/** A value as text. Never markup: views render it as a text node, and it's cut short so one huge value can't stall a table. */
export const valueText = (value: unknown): string => {
  if (value === null || value === undefined) return "";
  const text = typeof value === "string" ? value : Array.isArray(value) && value.every((v) => typeof v !== "object") ? value.map(String).join(", ") : typeof value === "object" ? JSON.stringify(value) : String(value as number | boolean);
  return text.length > MAX_CELL_CHARS ? `${text.slice(0, MAX_CELL_CHARS)}…` : text;
};

/** An attribute named `__proto__` or `constructor` is just a field, so this never reads through the prototype. */
export const attributeOf = (attributes: Readonly<Record<string, unknown>>, field: string): unknown => (Object.hasOwn(attributes, field) ? attributes[field] : undefined);

export type SortDirection = "asc" | "desc";

export interface EntitySort {
  readonly field: string;
  readonly direction: SortDirection;
}

type SortKey = number | string | null;

const keyOf = (value: unknown, kind: FieldKind): SortKey => {
  if (value === null || value === undefined) return null;
  if (kind === "number" && typeof value === "number") return value;
  return valueText(value).toLowerCase();
};

const compareKeys = (a: SortKey, b: SortKey): number => {
  if (typeof a === "number" && typeof b === "number") return a - b;
  const [x, y] = [String(a), String(b)];
  if (x === y) return 0;
  return x < y ? -1 : 1;
};

/**
 * Sorts the rows given and no others. The API lists entities by id only, so a screen has to say this covers only what's loaded.
 * Rows with no value stay last in both directions. Each key is computed once, as this may see hundreds of thousands of rows.
 */
export const sortLoadedRows = <Row>(rows: readonly Row[], valueOf: (row: Row) => unknown, kind: FieldKind, direction: SortDirection): readonly Row[] => {
  const sign = direction === "asc" ? 1 : -1;
  const keyed = rows.map((row, index) => ({ row, index, key: keyOf(valueOf(row), kind) }));
  keyed.sort((a, b) => {
    if (a.key === null || b.key === null) return a.key === b.key ? a.index - b.index : a.key === null ? 1 : -1;
    return sign * compareKeys(a.key, b.key) || a.index - b.index;
  });
  return keyed.map((k) => k.row);
};

const TIME_PATTERN = /^\d{4}-\d{2}-\d{2}(?:[Tt ]\d{2}:\d{2}(?::\d{2}(?:\.\d+)?)?(?:[Zz]|[+-]\d{2}:\d{2})?)?$/;

export interface EntityFilter {
  readonly field: string;
  readonly op: FilterOp;
  readonly value: string;
}

/** Why the server would refuse this value, so a person hears it before the request. Null when it's fine. */
export const filterProblem = (def: EntityFilterDef, op: FilterOp, value: string): string | null => {
  if (!def.ops.includes(op)) return `${def.label} can only be compared with "is".`;
  const trimmed = value.trim();
  if (trimmed === "") return `Give a value for ${def.label}.`;
  if (def.kind === "number" && !Number.isFinite(Number(trimmed))) return `${def.label} holds numbers.`;
  if (def.kind === "boolean" && trimmed !== "true" && trimmed !== "false") return `${def.label} holds true or false.`;
  if (def.kind === "time" && !TIME_PATTERN.test(trimmed)) return `${def.label} holds dates, such as 2026-01-31.`;
  return null;
};

export type ParsedValue = { readonly ok: true; readonly value: unknown } | { readonly ok: false; readonly problem: string };

const scalarOf = (field: SchemaField, raw: string): ParsedValue => {
  const text = raw.trim();
  switch (field.kind) {
    case "number": {
      const n = Number(text);
      if (text === "" || !Number.isFinite(n)) return { ok: false, problem: `${field.name} holds numbers.` };
      if (field.integer && !Number.isInteger(n)) return { ok: false, problem: `${field.name} holds whole numbers.` };
      return { ok: true, value: n };
    }
    case "boolean":
      return text === "true" || text === "false" ? { ok: true, value: text === "true" } : { ok: false, problem: `${field.name} holds true or false.` };
    case "time":
      return TIME_PATTERN.test(text) ? { ok: true, value: text } : { ok: false, problem: `${field.name} holds dates, such as 2026-01-31.` };
    default:
      return { ok: true, value: raw };
  }
};

/** What a form field's text means as a value for this field. The server's schema check still has the last word. */
export const parseFieldValue = (field: SchemaField, raw: string): ParsedValue => {
  if (field.kind === "other") {
    try {
      return { ok: true, value: JSON.parse(raw) as unknown };
    } catch {
      return { ok: false, problem: `${field.name} holds JSON.` };
    }
  }
  if (!field.isArray) return scalarOf(field, raw);
  const values: unknown[] = [];
  for (const part of raw.split(",").map((p) => p.trim()).filter((p) => p !== "")) {
    const one = scalarOf(field, part);
    if (!one.ok) return one;
    values.push(one.value);
  }
  return { ok: true, value: values };
};

/** The name to call an entity by: its first identity field with a value, and failing that its first alias. */
export const entityLabel = (attributes: Readonly<Record<string, unknown>>, aliases: readonly string[], fields: readonly SchemaField[]): string | null => {
  for (const f of fields) {
    if (!f.identity) continue;
    const text = valueText(attributeOf(attributes, f.name));
    if (text !== "") return text;
  }
  return aliases[0] ?? null;
};
