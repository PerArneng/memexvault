import { entityPath, parseFieldValue } from "@memex/core";
import type { EntityFieldInput, EntityProposal, SchemaField } from "@memex/core";
import type { EntityDetail, VaultId } from "@memex/types";
import { Button, Input } from "@memex/ui";
import { useMutation, useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { useEngine } from "../engine-context";
import { unwrap } from "../query";
import { QueryView } from "../shell/query-view";
import { AppLink, Callout, DEFAULT_NOTE, useGo } from "../wiki/wiki-common";
import { EMPTY_VALUE, ENTITY_KEYS, evidenceOf, ProposalNotice, typeLink, useEntityType, useRefreshEntities, ValueInput } from "./entity-common";
import type { ValueDraft } from "./entity-common";

const aliasesFrom = (text: string): readonly string[] => text.split("\n").map((a) => a.trim()).filter((a) => a !== "");

/** Reads what was typed: filled fields become inputs, and anything wrong stops the proposal and says what. */
const inputsFrom = (fields: readonly SchemaField[], values: Readonly<Record<string, ValueDraft>>, needRequired: boolean): { readonly inputs: readonly EntityFieldInput[]; readonly problems: readonly string[] } => {
  const inputs: EntityFieldInput[] = [];
  const problems: string[] = [];
  for (const field of fields) {
    const draft = values[field.name] ?? EMPTY_VALUE;
    const { evidence, problem } = evidenceOf(draft);
    if (problem !== null) problems.push(`${field.name}: ${problem}`);
    if (draft.raw.trim() === "") {
      if (needRequired && field.required) problems.push(`${field.name} is required.`);
      if (evidence.length > 0) problems.push(`${field.name} has evidence but no value.`);
      continue;
    }
    const parsed = parseFieldValue(field, draft.raw);
    if (parsed.ok) inputs.push({ field: field.name, value: parsed.value, evidence, replaces: null });
    else problems.push(parsed.problem);
  }
  return { inputs, problems };
};

export const NewEntity = ({ vault, type }: { readonly vault: VaultId; readonly type: string }) => {
  const engine = useEngine();
  const go = useGo();
  const refresh = useRefreshEntities(vault);
  const { query, view } = useEntityType(vault, type);
  const [values, setValues] = useState<Readonly<Record<string, ValueDraft>>>({});
  const [aliases, setAliases] = useState("");
  const [note, setNote] = useState(DEFAULT_NOTE);
  const [problems, setProblems] = useState<readonly string[]>([]);
  const [outcome, setOutcome] = useState<EntityProposal | null>(null);
  const submit = useMutation({
    mutationFn: (fields: readonly EntityFieldInput[]) => engine.createEntity(vault, { type, aliases: aliasesFrom(aliases), fields, rationale: note.trim() === "" ? DEFAULT_NOTE : note }),
    onSuccess: async (result) => {
      setOutcome(result);
      await refresh();
      if (result.kind === "saved" && result.entityId !== undefined) go(entityPath(vault, { kind: "entity", id: result.entityId }));
    },
  });
  return (
    <section aria-label={`New ${type}`} className="flex flex-col gap-3">
      <div className="flex items-center justify-between">
        <h2 className="text-lg font-medium break-words">New {type}</h2>
        <AppLink to={typeLink(vault, type)}>Back to {type}</AppLink>
      </div>
      <QueryView query={query} label="this type">
        {() =>
          view === null ? null : (
            <form className="flex flex-col gap-3" aria-label="Entity values" onSubmit={(e) => e.preventDefault()}>
              <div className="grid grid-cols-1 gap-2 lg:grid-cols-2">
                {view.fields.map((f) => <ValueInput key={f.name} field={f} idPrefix="new" value={values[f.name] ?? EMPTY_VALUE} onChange={(next) => setValues({ ...values, [f.name]: next })} />)}
              </div>
              <AliasBox value={aliases} onChange={setAliases} />
              <Input label="Why" value={note} onChange={(e) => setNote(e.target.value)} />
              {problems.length > 0 && <Callout tone="warning" title="Fix these first."><ul className="list-disc pl-4">{problems.map((p) => <li key={p}>{p}</li>)}</ul></Callout>}
              {outcome !== null && <ProposalNotice vault={vault} outcome={outcome} what="This entity" />}
              <div>
                <Button variant="primary" disabled={submit.isPending || outcome?.kind === "pending"} onClick={() => {
                  const read = inputsFrom(view.fields, values, true);
                  setProblems(read.problems);
                  if (read.problems.length === 0) submit.mutate(read.inputs);
                }}>Propose this entity</Button>
              </div>
            </form>
          )
        }
      </QueryView>
    </section>
  );
};

const AliasBox = ({ value, onChange }: { readonly value: string; readonly onChange: (next: string) => void }) => (
  <div className="flex flex-col gap-1">
    <label htmlFor="entity-aliases" className="text-xs text-text-muted">Aliases, one per line</label>
    <textarea id="entity-aliases" rows={3} className="rounded-md border border-border-strong bg-surface-raised p-2 text-sm text-text" value={value} onChange={(e) => onChange(e.target.value)} />
  </div>
);

const EditForm = ({ vault, entity }: { readonly vault: VaultId; readonly entity: EntityDetail }) => {
  const engine = useEngine();
  const refresh = useRefreshEntities(vault);
  const { query, view } = useEntityType(vault, entity.typeName);
  const [values, setValues] = useState<Readonly<Record<string, ValueDraft>>>({});
  const [replacing, setReplacing] = useState<ReadonlySet<string>>(new Set());
  const [aliases, setAliases] = useState(entity.aliases.join("\n"));
  const [note, setNote] = useState(DEFAULT_NOTE);
  const [problems, setProblems] = useState<readonly string[]>([]);
  const [outcome, setOutcome] = useState<EntityProposal | null>(null);
  const currentOf = (name: string) => entity.fields.find((f) => f.field === name);
  const submit = useMutation({
    mutationFn: (fields: readonly EntityFieldInput[]) => {
      const next = aliasesFrom(aliases);
      const changed = next.join("\n") !== entity.aliases.join("\n");
      const chosen = Object.fromEntries(fields.flatMap((f): [string, string][] => {
        const current = currentOf(f.field);
        return current !== undefined && replacing.has(f.field) ? [[f.field, current.current.assertionId]] : [];
      }));
      return engine.editEntity(vault, entity.id, { baseVersion: entity.version, aliases: changed ? next : null, fields, replacing: chosen, rationale: note.trim() === "" ? DEFAULT_NOTE : note });
    },
    onSuccess: async (result) => {
      setOutcome(result);
      await refresh();
      if (result.kind === "saved") setValues({});
    },
  });
  return (
    <QueryView query={query} label="this type">
      {() =>
        view === null ? null : (
          <form className="flex flex-col gap-3" aria-label="Entity values" onSubmit={(e) => e.preventDefault()}>
            <p className="text-sm text-text-muted">Fill in only what changes. A new value goes beside the others on its field, so disagreements stay visible, unless you say it replaces the current one.</p>
            <div className="grid grid-cols-1 gap-2 lg:grid-cols-2">
              {view.fields.map((f) => {
                const current = currentOf(f.name);
                return (
                  <div key={f.name} className="flex flex-col gap-1">
                    <ValueInput field={f} idPrefix="edit" value={values[f.name] ?? EMPTY_VALUE} onChange={(next) => setValues({ ...values, [f.name]: next })} />
                    {current !== undefined && (
                      <label className="flex items-center gap-1 text-xs text-text-muted">
                        <input type="checkbox" checked={replacing.has(f.name)} onChange={(e) => {
                          const next = new Set(replacing);
                          if (e.target.checked) next.add(f.name);
                          else next.delete(f.name);
                          setReplacing(next);
                        }} /> Replaces the current value of {f.name}
                      </label>
                    )}
                  </div>
                );
              })}
            </div>
            <AliasBox value={aliases} onChange={setAliases} />
            <Input label="Why" value={note} onChange={(e) => setNote(e.target.value)} />
            {problems.length > 0 && <Callout tone="warning" title="Fix these first."><ul className="list-disc pl-4">{problems.map((p) => <li key={p}>{p}</li>)}</ul></Callout>}
            {outcome !== null && <ProposalNotice vault={vault} outcome={outcome} what="This edit" done={<AppLink to={entityPath(vault, { kind: "entity", id: entity.id })}>See the entity</AppLink>} />}
            <div>
              <Button variant="primary" disabled={submit.isPending} onClick={() => {
                const read = inputsFrom(view.fields, values, false);
                const aliasChange = aliasesFrom(aliases).join("\n") !== entity.aliases.join("\n");
                const wrong = read.inputs.length === 0 && !aliasChange ? ["There's nothing to propose yet."] : read.problems;
                setProblems(wrong);
                if (wrong.length === 0) submit.mutate(read.inputs);
              }}>Propose this edit</Button>
            </div>
          </form>
        )
      }
    </QueryView>
  );
};

export const EditEntity = ({ vault, id }: { readonly vault: VaultId; readonly id: string }) => {
  const engine = useEngine();
  const query = useQuery({ queryKey: ENTITY_KEYS.entity(vault, id), queryFn: async () => unwrap(await engine.getEntity(vault, id)) });
  return (
    <section aria-label="Edit entity" className="flex flex-col gap-3">
      <div className="flex items-center justify-between">
        <h2 className="text-lg font-medium">Edit</h2>
        <AppLink to={entityPath(vault, { kind: "entity", id })}>Back to the entity</AppLink>
      </div>
      <QueryView query={query} label="this entity">{(entity) => <EditForm key={entity.version} vault={vault} entity={entity} />}</QueryView>
    </section>
  );
};
