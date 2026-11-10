import type { EntityDetail, Neighbor } from "@memex/types";
import { MAX_SCHEMA_FIELDS, valueText } from "./entity-schema";

/** What the form offers. A schema with rules the form doesn't name keeps them, as `schemaFromDraft` builds on the field's original description. */
export const DRAFT_KINDS = ["text", "number", "integer", "boolean", "date", "date-time", "text-list", "object"] as const;
export type DraftKind = (typeof DRAFT_KINDS)[number];

export const DRAFT_KIND_LABELS: Readonly<Record<DraftKind, string>> = {
  text: "Text",
  number: "Number",
  integer: "Whole number",
  boolean: "True or false",
  date: "Date",
  "date-time": "Date and time",
  "text-list": "List of text",
  object: "Other (JSON)",
};

export interface DraftField {
  /** Stable while its name is edited. */
  readonly key: string;
  readonly name: string;
  readonly kind: DraftKind;
  readonly required: boolean;
  readonly searchable: boolean;
  readonly filterable: boolean;
  readonly identity: boolean;
  readonly description: string;
  /** The field's description in the schema this started from, so rules the form doesn't show survive. */
  readonly original: Readonly<Record<string, unknown>> | null;
  /** The kind it started as, so the original's rules go only if the kind changes. */
  readonly originalKind: DraftKind | null;
}

export interface TypeDraft {
  readonly name: string;
  readonly fields: readonly DraftField[];
  /** Everything at the schema's top level that the form doesn't show. */
  readonly outer: Readonly<Record<string, unknown>>;
}

const isRecord = (value: unknown): value is Readonly<Record<string, unknown>> => typeof value === "object" && value !== null && !Array.isArray(value);

const kindOf = (def: Readonly<Record<string, unknown>>): DraftKind => {
  const t = Array.isArray(def.type) ? (def.type as readonly unknown[]).find((x) => x !== "null") : def.type;
  if (t === "string") return def.format === "date" ? "date" : def.format === "date-time" ? "date-time" : "text";
  if (t === "number") return "number";
  if (t === "integer") return "integer";
  if (t === "boolean") return "boolean";
  if (t === "array" && isRecord(def.items) && def.items.type === "string") return "text-list";
  return "object";
};

const rulesOf = (kind: DraftKind): Readonly<Record<string, unknown>> => {
  switch (kind) {
    case "text":
      return { type: "string" };
    case "number":
      return { type: "number" };
    case "integer":
      return { type: "integer" };
    case "boolean":
      return { type: "boolean" };
    case "date":
      return { type: "string", format: "date" };
    case "date-time":
      return { type: "string", format: "date-time" };
    case "text-list":
      return { type: "array", items: { type: "string" } };
    case "object":
      return { type: "object" };
  }
};

export const blankField = (key: string): DraftField => ({ key, name: "", kind: "text", required: false, searchable: false, filterable: false, identity: false, description: "", original: null, originalKind: null });

/** A new type starts closed, as the server requires, with one field to fill in. */
export const blankDraft = (name: string, firstKey: string): TypeDraft => ({
  name,
  fields: [{ ...blankField(firstKey), name: "name", required: true, searchable: true, filterable: true, identity: true }],
  outer: { type: "object", additionalProperties: false },
});

export const draftFromSchema = (name: string, schema: unknown): TypeDraft => {
  const record = isRecord(schema) ? schema : {};
  const properties = isRecord(record.properties) ? record.properties : {};
  const required = new Set(Array.isArray(record.required) ? record.required : []);
  const fields = Object.keys(properties)
    .sort()
    .slice(0, MAX_SCHEMA_FIELDS)
    .flatMap((field): DraftField[] => {
      const def = properties[field];
      if (!isRecord(def)) return [];
      const kind = kindOf(def);
      return [{
        key: field,
        name: field,
        kind,
        required: required.has(field),
        searchable: def.searchable === true,
        filterable: def.filterable === true,
        identity: def.identity === true,
        description: typeof def.description === "string" ? def.description : "",
        original: def,
        originalKind: kind,
      }];
    });
  const outer = Object.fromEntries(Object.entries(record).filter(([k]) => k !== "properties" && k !== "required"));
  return { name, fields, outer };
};

const NAME_PATTERN = /^[A-Za-z][A-Za-z0-9_]*$/;
const MAX_NAME_BYTES = 64;

const nameProblem = (name: string): boolean => !NAME_PATTERN.test(name) || name.length > MAX_NAME_BYTES;

/** The checks a form can make at once, in the server's own terms. The preview still has the last word. */
export const draftProblems = (draft: TypeDraft): readonly string[] => {
  const out: string[] = [];
  if (nameProblem(draft.name)) out.push("A type's name starts with a letter, has only letters, digits and _, and is at most 64 characters.");
  if (draft.fields.length === 0) out.push("A type needs at least one field.");
  if (draft.fields.length > MAX_SCHEMA_FIELDS) out.push(`A type can have at most ${String(MAX_SCHEMA_FIELDS)} fields.`);
  const seen = new Set<string>();
  for (const f of draft.fields) {
    if (nameProblem(f.name)) out.push(`The field name "${f.name}" isn't allowed: it must start with a letter and have only letters, digits and _.`);
    if (seen.has(f.name)) out.push(`Two fields are named ${f.name}.`);
    seen.add(f.name);
    if (f.kind === "object" && (f.searchable || f.filterable || f.identity)) out.push(`${f.name} holds JSON, so it can't be searchable, filterable or an identity.`);
    if (f.identity && f.kind !== "text") out.push(`${f.name} is an identity, so it must hold text.`);
  }
  return out;
};

const fieldSchema = (f: DraftField): Readonly<Record<string, unknown>> => {
  // The annotations and description come from the form, so an unticked box removes one.
  const kept = f.original !== null && f.originalKind === f.kind
    ? Object.fromEntries(Object.entries(f.original).filter(([k]) => !["searchable", "filterable", "identity", "description"].includes(k)))
    : rulesOf(f.kind);
  return {
    ...kept,
    ...(f.description.trim() === "" ? {} : { description: f.description }),
    ...(f.searchable ? { searchable: true } : {}),
    ...(f.filterable ? { filterable: true } : {}),
    ...(f.identity ? { identity: true } : {}),
  };
};

export const schemaFromDraft = (draft: TypeDraft): Readonly<Record<string, unknown>> => {
  const required = draft.fields.filter((f) => f.required).map((f) => f.name);
  return {
    ...draft.outer,
    ...(required.length === 0 ? {} : { required }),
    properties: Object.fromEntries(draft.fields.map((f) => [f.name, fieldSchema(f)])),
  };
};

export interface ApprovalText {
  readonly tone: "success" | "warning" | "danger";
  readonly title: string;
  readonly detail: string;
}

/** What a preview means for the person about to submit. An unknown word from a newer server reads as needing review, the safe side. */
export const approvalText = (decision: string, problem: string | null): ApprovalText => {
  if (problem !== null || decision === "reject") return { tone: "danger", title: "This would be refused.", detail: problem ?? "The server's rules refuse this change." };
  if (decision === "auto_commit") return { tone: "success", title: "No approval needed.", detail: "It takes effect as soon as you submit it." };
  return { tone: "warning", title: "A person has to approve this.", detail: "It waits in Review, and the type doesn't change until it's approved." };
};

export type MergeOutcome = "same" | "from_source" | "stays" | "conflict";

export interface MergeRow {
  readonly field: string;
  readonly targetValue: string | null;
  readonly sourceValue: string | null;
  readonly outcome: MergeOutcome;
}

export interface MergePreview {
  readonly rows: readonly MergeRow[];
  readonly aliases: readonly string[];
  /** `source` then leads to `target`, while the merge stands. */
  readonly redirect: { readonly from: string; readonly to: string };
  readonly sameType: boolean;
}

/** What a merge would show, computed from what both entities hold now. It predicts the list; the server decides each field's current value by its own rule when it's approved. */
export const mergePreview = (source: EntityDetail, target: EntityDetail): MergePreview => {
  const of = (e: EntityDetail) => new Map(e.fields.map((f) => [f.field, valueText(f.value)]));
  const [from, to] = [of(source), of(target)];
  const names = [...new Set([...from.keys(), ...to.keys()])].sort();
  const rows = names.map((field): MergeRow => {
    const sourceValue = from.get(field) ?? null;
    const targetValue = to.get(field) ?? null;
    const outcome: MergeOutcome = sourceValue === null ? "stays" : targetValue === null ? "from_source" : sourceValue === targetValue ? "same" : "conflict";
    return { field, sourceValue, targetValue, outcome };
  });
  return {
    rows,
    aliases: [...new Set([...target.aliases, ...source.aliases])],
    redirect: { from: source.id, to: target.id },
    sameType: source.typeName === target.typeName,
  };
};

export const MERGE_OUTCOME_TEXT: Readonly<Record<MergeOutcome, string>> = {
  same: "Both agree",
  from_source: "Comes from the merged entity",
  stays: "Stays as it is",
  conflict: "Conflict: both stay visible, and the usual rule picks one",
};

export interface SplitPreview {
  readonly moved: readonly { readonly field: string; readonly value: string }[];
  readonly staying: readonly { readonly field: string; readonly value: string }[];
  readonly movedAliases: readonly string[];
  readonly stayingAliases: readonly string[];
}

export const splitPreview = (entity: EntityDetail, movedClaims: ReadonlySet<string>, movedAliases: ReadonlySet<string>): SplitPreview => {
  const claims = entity.fields.flatMap((f) => [f.current, ...f.conflicts, ...f.agreeing].map((c) => ({ field: f.field, id: c.assertionId, value: valueText(c.value) })));
  const shown = ({ field, value }: { readonly field: string; readonly value: string }) => ({ field, value });
  return {
    moved: claims.filter((c) => movedClaims.has(c.id)).map(shown),
    staying: claims.filter((c) => !movedClaims.has(c.id)).map(shown),
    movedAliases: entity.aliases.filter((a) => movedAliases.has(a)),
    stayingAliases: entity.aliases.filter((a) => !movedAliases.has(a)),
  };
};

/** Claims the graph panel asks about beyond the entity itself. An entity with more gets a note rather than a burst of requests. */
export const MAX_GRAPH_CLAIMS = 24;
export const MAX_GRAPH_NODES = 40;

/** Each field's claims, current first, up to the limit. */
export const graphClaims = (entity: EntityDetail): { readonly claims: readonly { readonly id: string; readonly field: string }[]; readonly truncated: boolean } => {
  const all = entity.fields.flatMap((f) => [f.current, ...f.conflicts, ...f.agreeing].map((c) => ({ id: c.assertionId, field: f.field })));
  return { claims: all.slice(0, MAX_GRAPH_CLAIMS), truncated: all.length > MAX_GRAPH_CLAIMS };
};

export interface GraphLink {
  readonly edgeId: string;
  readonly kind: string;
  readonly direction: string;
  /** The field whose claim has this link, or null for a link at the entity itself. */
  readonly viaField: string | null;
}

export interface GraphNode {
  readonly key: string;
  /** Null is a link to a slug no page holds. */
  readonly objectId: string | null;
  readonly title: string | null;
  readonly links: readonly GraphLink[];
  readonly x: number;
  readonly y: number;
}

export interface EntityGraph {
  readonly nodes: readonly GraphNode[];
  readonly truncated: boolean;
}

const VIEW_RADIUS = 38;
const TURN = 2 * Math.PI;

/** Merges what each object's neighborhood said into one set of nodes around the entity, placed on a circle in percent of the panel. Its own claims don't count as neighbors of themselves. */
export const entityGraph = (entityId: string, answers: readonly { readonly viaField: string | null; readonly items: readonly Neighbor[] }[], claimIds: ReadonlySet<string>): EntityGraph => {
  const found = new Map<string, { objectId: string | null; title: string | null; links: GraphLink[] }>();
  for (const { viaField, items } of answers) {
    for (const n of items) {
      if (n.neighbor === entityId || (n.neighbor !== null && claimIds.has(n.neighbor))) continue;
      const key = n.neighbor ?? `unresolved:${n.to}`;
      const known = found.get(key) ?? { objectId: n.neighbor, title: n.title, links: [] };
      if (!known.links.some((l) => l.edgeId === n.edgeId)) known.links.push({ edgeId: n.edgeId, kind: n.kind, direction: n.direction, viaField });
      found.set(key, known);
    }
  }
  const entries = [...found.entries()].sort(([a], [b]) => (a < b ? -1 : 1));
  const kept = entries.slice(0, MAX_GRAPH_NODES);
  return {
    truncated: entries.length > MAX_GRAPH_NODES,
    nodes: kept.map(([key, node], i) => {
      const angle = (TURN * i) / kept.length - TURN / 4;
      return { key, ...node, x: 50 + VIEW_RADIUS * Math.cos(angle), y: 50 + VIEW_RADIUS * Math.sin(angle) };
    }),
  };
};
