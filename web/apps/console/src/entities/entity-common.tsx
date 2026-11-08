import { entityPath, parseEntityRoute, requestIdText, schemaView, sectionPath } from "@memex/core";
import type { EntityFieldInput, EntityProposal, EntityRoute, SchemaField, SchemaView } from "@memex/core";
import type { EntityType, VaultId } from "@memex/types";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { useCallback, useMemo } from "react";
import type { ReactNode } from "react";
import { useEngine } from "../engine-context";
import { QUERY_KEYS, unwrap } from "../query";
import { AppLink, Callout, failureText } from "../wiki/wiki-common";

/** Everything about one vault's entities, so a write can refresh all of it. Here rather than in `query.ts`, as only this screen uses them. */
export const ENTITY_KEYS = {
  all: (vault: string) => ["entities", vault] as const,
  types: (vault: string) => ["entities", vault, "types"] as const,
  versions: (vault: string, type: string) => ["entities", vault, "versions", type] as const,
  rows: (vault: string, type: string, filters: string) => ["entities", vault, "rows", type, filters] as const,
  entity: (vault: string, id: string) => ["entities", vault, "entity", id] as const,
  redirect: (vault: string, id: string) => ["entities", vault, "redirect", id] as const,
  resolve: (vault: string, type: string | null, mention: string) => ["entities", vault, "resolve", type, mention] as const,
  sameAs: (vault: string, type: string) => ["entities", vault, "same-as", type] as const,
  graph: (vault: string, id: string, version: number) => ["entities", vault, "graph", id, version] as const,
};

export const SELECT_CLASS = "h-8 rounded-md border border-border-strong bg-surface-raised px-2 text-sm text-text";
export const FIELD_LABEL = "text-xs text-text-muted";

export const routeFrom = (rest: readonly string[], search: string): EntityRoute => {
  const query = new URLSearchParams(search);
  return parseEntityRoute(rest, (name) => query.getAll(name));
};

export const useRefreshEntities = (vault: VaultId) => {
  const queryClient = useQueryClient();
  return useCallback(async () => {
    await queryClient.invalidateQueries({ queryKey: ENTITY_KEYS.all(vault) });
    await queryClient.invalidateQueries({ queryKey: QUERY_KEYS.objects(vault) });
  }, [queryClient, vault]);
};

/** A type's versions, oldest first, and what its newest schema says. Nothing about a type is known to the console except through this. */
export const useEntityType = (vault: VaultId, type: string) => {
  const engine = useEngine();
  const query = useQuery({ queryKey: ENTITY_KEYS.versions(vault, type), queryFn: async () => unwrap(await engine.listEntityTypeVersions(vault, type)) });
  const latest: EntityType | null = query.data?.at(-1) ?? null;
  const view: SchemaView | null = useMemo(() => (latest === null ? null : schemaView(latest.schema)), [latest]);
  return { query, latest, view };
};

export const SchemaProblems = ({ problems }: { readonly problems: readonly string[] }) =>
  problems.length === 0 ? null : (
    <Callout tone="warning" title="Part of this type's schema can't be shown.">
      <ul className="list-disc pl-4">
        {problems.map((p) => <li key={p}>{p}</li>)}
      </ul>
    </Callout>
  );

/** What became of a proposal, in the page. Every word about review comes from the server's answer. */
export const ProposalNotice = ({ vault, outcome, what, done }: { readonly vault: VaultId; readonly outcome: EntityProposal; readonly what: string; readonly done?: ReactNode }) => {
  switch (outcome.kind) {
    case "saved":
      return <Callout tone="info" title={`${what} is saved.`}>{done}</Callout>;
    case "pending":
      return (
        <div data-proposal="pending" data-changeset={outcome.changesetId ?? ""}>
          <Callout tone="info" title={`${what} is waiting for review.`}>
            Nothing changes until a person approves it. <AppLink to={sectionPath(vault, "review", outcome.changesetId ?? undefined)}>Open it in Review</AppLink>
          </Callout>
        </div>
      );
    case "refused":
      return <Callout tone="danger" title={`${what} was refused.`}>{outcome.reasons.join(" ")}</Callout>;
    case "stale":
      return <Callout tone="warning" title="This changed since you opened it, so nothing was done.">It has been reloaded. Look it over and try again.</Callout>;
    case "failed":
      return <Callout tone="danger" title={failureText(outcome.failure)}>{requestIdText(outcome.failure) === null ? "If the server took it, trying again won't apply it twice." : null}</Callout>;
  }
};

export interface ValueDraft {
  readonly raw: string;
  readonly segmentId: string;
  readonly quote: string;
}

export const EMPTY_VALUE: ValueDraft = { raw: "", segmentId: "", quote: "" };

/** Evidence goes in by segment and quote, which is what the server checks. A half-filled pair is a mistake, so it isn't sent as no evidence. */
export const evidenceOf = (draft: ValueDraft): { readonly evidence: EntityFieldInput["evidence"]; readonly problem: string | null } => {
  const [segmentId, quote] = [draft.segmentId.trim(), draft.quote.trim()];
  if (segmentId === "" && quote === "") return { evidence: [], problem: null };
  if (segmentId === "" || quote === "") return { evidence: [], problem: "Evidence needs both a segment and a quote." };
  return { evidence: [{ segmentId, quote }], problem: null };
};

const inputTypeOf = (field: SchemaField): string => {
  if (field.isArray) return "text";
  if (field.kind === "number") return "number";
  return field.kind === "time" && field.format === "date" ? "date" : "text";
};

const hintOf = (field: SchemaField): string | null => {
  if (field.isArray) return "Separate values with commas.";
  if (field.kind === "other") return "JSON.";
  if (field.kind === "time" && field.format !== "date") return "Such as 2026-01-31T12:00:00Z.";
  return null;
};

/** One field's input, generated from its schema. */
export const ValueInput = ({ field, idPrefix, value, onChange }: { readonly field: SchemaField; readonly idPrefix: string; readonly value: ValueDraft; readonly onChange: (next: ValueDraft) => void }) => {
  const id = `${idPrefix}-${field.name}`;
  const hint = hintOf(field);
  const choices = field.kind === "boolean" ? ["true", "false"] : field.isArray ? null : field.enumValues;
  return (
    <div className="flex flex-col gap-2 rounded-md border border-border p-2" data-field-input={field.name}>
      <div className="flex flex-col gap-1">
        <label htmlFor={id} className={FIELD_LABEL}>{field.name}{field.required ? " (required)" : ""}</label>
        {choices !== null && choices.length > 0 ? (
          <select id={id} className={SELECT_CLASS} value={value.raw} onChange={(e) => onChange({ ...value, raw: e.target.value })}>
            <option value="">No value</option>
            {choices.map((c) => <option key={c} value={c}>{c}</option>)}
          </select>
        ) : (
          <input id={id} type={inputTypeOf(field)} step="any" className="h-8 rounded-md border border-border-strong bg-surface-raised px-2 text-sm text-text" value={value.raw} onChange={(e) => onChange({ ...value, raw: e.target.value })} />
        )}
        {field.description !== null && <p className="text-xs text-text-muted">{field.description}</p>}
        {hint !== null && <p className="text-xs text-text-muted">{hint}</p>}
      </div>
      <details className="text-xs">
        <summary className="cursor-pointer text-text-muted">Evidence for {field.name}</summary>
        <div className="mt-2 grid grid-cols-2 gap-2">
          <div className="flex flex-col gap-1">
            <label htmlFor={`${id}-segment`} className={FIELD_LABEL}>Segment id</label>
            <input id={`${id}-segment`} className="h-8 rounded-md border border-border-strong bg-surface-raised px-2 font-mono text-xs text-text" value={value.segmentId} onChange={(e) => onChange({ ...value, segmentId: e.target.value })} />
          </div>
          <div className="flex flex-col gap-1">
            <label htmlFor={`${id}-quote`} className={FIELD_LABEL}>Quote, word for word</label>
            <input id={`${id}-quote`} className="h-8 rounded-md border border-border-strong bg-surface-raised px-2 text-sm text-text" value={value.quote} onChange={(e) => onChange({ ...value, quote: e.target.value })} />
          </div>
        </div>
      </details>
    </div>
  );
};

export const typeLink = (vault: VaultId, type: string) => entityPath(vault, { kind: "table", type, filters: [], columns: null, sort: null, find: "" });
export { failureText };
