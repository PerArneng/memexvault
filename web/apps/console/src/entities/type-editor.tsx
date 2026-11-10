import { approvalText, blankDraft, blankField, DRAFT_KIND_LABELS, DRAFT_KINDS, draftFromSchema, draftProblems, entityPath, requestIdText, schemaFromDraft } from "@memex/core";
import type { DraftField, DraftKind, EntityProposal, TypeDraft } from "@memex/core";
import type { EntityTypePreview, VaultId } from "@memex/types";
import { Badge, Button, Input } from "@memex/ui";
import { useMutation } from "@tanstack/react-query";
import { useId, useMemo, useRef, useState } from "react";
import { useEngine } from "../engine-context";
import { FailureError, unwrap } from "../query";
import { QueryView } from "../shell/query-view";
import { AppLink, Callout, DEFAULT_NOTE, useGo } from "../wiki/wiki-common";
import { FIELD_LABEL, ProposalNotice, SELECT_CLASS, typeLink, useEntityType, useRefreshEntities } from "./entity-common";

const Tick = ({ id, label, checked, onChange }: { readonly id: string; readonly label: string; readonly checked: boolean; readonly onChange: (next: boolean) => void }) => (
  <label htmlFor={id} className="inline-flex items-center gap-1 text-xs text-text-muted">
    <input id={id} type="checkbox" checked={checked} onChange={(e) => onChange(e.target.checked)} /> {label}
  </label>
);

const FieldRow = ({ field, onChange, onRemove }: { readonly field: DraftField; readonly onChange: (next: DraftField) => void; readonly onRemove: () => void }) => {
  const id = useId();
  return (
    <li className="flex flex-wrap items-end gap-3 rounded-md border border-border p-2" data-draft-field={field.key}>
      <div className="flex flex-col gap-1">
        <label htmlFor={`${id}-name`} className={FIELD_LABEL}>Field name</label>
        <input id={`${id}-name`} className={SELECT_CLASS} value={field.name} onChange={(e) => onChange({ ...field, name: e.target.value })} />
      </div>
      <div className="flex flex-col gap-1">
        <label htmlFor={`${id}-kind`} className={FIELD_LABEL}>Holds</label>
        <select id={`${id}-kind`} className={SELECT_CLASS} value={field.kind} onChange={(e) => onChange({ ...field, kind: e.target.value as DraftKind })}>
          {DRAFT_KINDS.map((k) => <option key={k} value={k}>{DRAFT_KIND_LABELS[k]}</option>)}
        </select>
      </div>
      <div className="flex min-w-48 flex-1 flex-col gap-1">
        <label htmlFor={`${id}-description`} className={FIELD_LABEL}>Description</label>
        <input id={`${id}-description`} className={SELECT_CLASS} value={field.description} onChange={(e) => onChange({ ...field, description: e.target.value })} />
      </div>
      <fieldset className="flex items-center gap-3 pb-1.5">
        <legend className="sr-only">About {field.name}</legend>
        <Tick id={`${id}-required`} label="Required" checked={field.required} onChange={(required) => onChange({ ...field, required })} />
        <Tick id={`${id}-searchable`} label="Searchable" checked={field.searchable} onChange={(searchable) => onChange({ ...field, searchable })} />
        <Tick id={`${id}-filterable`} label="Filterable" checked={field.filterable} onChange={(filterable) => onChange({ ...field, filterable })} />
        <Tick id={`${id}-identity`} label="Identity" checked={field.identity} onChange={(identity) => onChange({ ...field, identity })} />
      </fieldset>
      <Button variant="ghost" onClick={onRemove}>Remove {field.name === "" ? "this field" : field.name}</Button>
    </li>
  );
};

const PreviewPanel = ({ preview }: { readonly preview: EntityTypePreview }) => {
  const approval = approvalText(preview.decision, preview.problem);
  return (
    <section aria-label="Preview" data-preview-change={preview.change} data-preview-decision={preview.decision} className="flex flex-col gap-2 rounded-lg border border-border-strong bg-surface-raised p-3">
      <h3 className="flex flex-wrap items-center gap-2 font-medium">
        Preview <Badge tone={preview.change === "breaking" ? "warning" : preview.change === "additive" ? "success" : "info"}>{preview.change}</Badge>
        <span className="text-sm font-normal text-text-muted">{preview.currentVersion === null ? `Becomes version ${String(preview.nextVersion)}` : `Version ${String(preview.currentVersion)} to ${String(preview.nextVersion)}`}</span>
      </h3>
      {preview.reasons.length > 0 && <ul className="list-disc pl-4 text-sm text-text-muted">{preview.reasons.map((r) => <li key={r}>{r}</li>)}</ul>}
      <Callout tone={approval.tone === "success" ? "info" : approval.tone} title={approval.title}>{approval.detail}</Callout>
    </section>
  );
};

/** The form for a new type or a type's next version. It shows what the server says a change is before anything can be submitted. */
const TypeForm = ({ vault, initial, baseVersion, nameLocked }: { readonly vault: VaultId; readonly initial: TypeDraft; readonly baseVersion: number | null; readonly nameLocked: boolean }) => {
  const engine = useEngine();
  const go = useGo();
  const refresh = useRefreshEntities(vault);
  const [draft, setDraft] = useState(initial);
  const [note, setNote] = useState(DEFAULT_NOTE);
  const [outcome, setOutcome] = useState<EntityProposal | null>(null);
  const nextKey = useRef(0);
  const problems = useMemo(() => draftProblems(draft), [draft]);
  const schema = useMemo(() => schemaFromDraft(draft), [draft]);
  const schemaText = useMemo(() => JSON.stringify(schema), [schema]);
  const preview = useMutation({ mutationFn: async () => ({ forSchema: schemaText, answer: unwrap(await engine.previewEntityType(vault, draft.name, schema)) }) });
  // A preview answers for one schema, so any edit asks for a new one.
  const current = preview.data?.forSchema === schemaText ? preview.data.answer : null;
  const submit = useMutation({
    mutationFn: () => engine.submitEntityType(vault, { name: draft.name, baseVersion, schema, rationale: note.trim() === "" ? DEFAULT_NOTE : note }),
    onSuccess: async (result) => {
      setOutcome(result);
      if (result.kind === "saved") {
        await refresh();
        go(typeLink(vault, draft.name));
      }
    },
  });
  const setField = (key: string, next: DraftField) => setDraft({ ...draft, fields: draft.fields.map((f) => (f.key === key ? next : f)) });
  const blocked = current === null || current.problem !== null || current.decision === "reject" || problems.length > 0;
  const previewFailure = preview.error instanceof FailureError ? preview.error.failure : null;
  return (
    <form className="flex flex-col gap-3" aria-label="Type schema" onSubmit={(e) => e.preventDefault()}>
      <div className="w-72"><Input label="Type name" value={draft.name} readOnly={nameLocked} onChange={(e) => setDraft({ ...draft, name: e.target.value })} /></div>
      <ul className="flex flex-col gap-2" aria-label="Fields">
        {draft.fields.map((f) => <FieldRow key={f.key} field={f} onChange={(next) => setField(f.key, next)} onRemove={() => setDraft({ ...draft, fields: draft.fields.filter((x) => x.key !== f.key) })} />)}
      </ul>
      <div>
        <Button onClick={() => {
          nextKey.current += 1;
          setDraft({ ...draft, fields: [...draft.fields, blankField(`added-${String(nextKey.current)}`)] });
        }}>Add a field</Button>
      </div>
      {problems.length > 0 && (
        <Callout tone="warning" title="Fix these first.">
          <ul className="list-disc pl-4">{problems.map((p) => <li key={p}>{p}</li>)}</ul>
        </Callout>
      )}
      {previewFailure !== null && <Callout tone="danger" title={previewFailure.message}>{requestIdText(previewFailure)}</Callout>}
      {current !== null && <PreviewPanel preview={current} />}
      <Input label="Why" value={note} onChange={(e) => setNote(e.target.value)} />
      {outcome !== null && <ProposalNotice vault={vault} outcome={outcome} what={`This change to ${draft.name}`} />}
      <div className="flex flex-wrap items-center gap-2">
        <Button disabled={problems.length > 0 || preview.isPending} onClick={() => preview.mutate()}>Preview the change</Button>
        <Button variant="primary" disabled={blocked || submit.isPending || outcome?.kind === "pending"} onClick={() => submit.mutate()}>Submit</Button>
        {current === null && <span className="text-xs text-text-muted">Preview it to see what approval it needs, and then it can be submitted.</span>}
      </div>
    </form>
  );
};

export const NewType = ({ vault }: { readonly vault: VaultId }) => {
  const initial = useMemo(() => blankDraft("", "first"), []);
  return (
    <section aria-label="New type" className="flex flex-col gap-3">
      <div className="flex items-center justify-between">
        <h2 className="text-lg font-medium">New type</h2>
        <AppLink to={entityPath(vault, { kind: "types" })}>Back to types</AppLink>
      </div>
      <TypeForm vault={vault} initial={initial} baseVersion={null} nameLocked={false} />
    </section>
  );
};

export const EditType = ({ vault, type }: { readonly vault: VaultId; readonly type: string }) => {
  const { query, latest } = useEntityType(vault, type);
  return (
    <section aria-label={`Change ${type}`} className="flex flex-col gap-3">
      <div className="flex items-center justify-between">
        <h2 className="text-lg font-medium break-words">Change {type}</h2>
        <AppLink to={entityPath(vault, { kind: "type", type })}>Back to its versions</AppLink>
      </div>
      <QueryView query={query} label="this type">
        {() => (latest === null ? null : <TypeForm key={latest.version} vault={vault} initial={draftFromSchema(type, latest.schema)} baseVersion={latest.version} nameLocked={true} />)}
      </QueryView>
    </section>
  );
};
