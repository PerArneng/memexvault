import { entityLabel, entityPath, MERGE_OUTCOME_TEXT, mergePreview, parseFieldValue, splitPreview, valueText } from "@memex/core";
import type { EntityFieldInput, EntityProposal } from "@memex/core";
import type { EntityDetail, VaultId } from "@memex/types";
import { Badge, Button, EmptyState, Input } from "@memex/ui";
import { useMutation, useQuery } from "@tanstack/react-query";
import { useMemo, useState } from "react";
import { useEngine } from "../engine-context";
import { unwrap } from "../query";
import { QueryView } from "../shell/query-view";
import { AppLink, Callout, DEFAULT_NOTE, LINK_BUTTON, useGo } from "../wiki/wiki-common";
import { EMPTY_VALUE, ENTITY_KEYS, evidenceOf, ProposalNotice, typeLink, useEntityType, useRefreshEntities, ValueInput } from "./entity-common";
import type { ValueDraft } from "./entity-common";

const useEntity = (vault: VaultId, id: string, enabled = true) => {
  const engine = useEngine();
  return useQuery({ queryKey: ENTITY_KEYS.entity(vault, id), enabled: enabled && id !== "", queryFn: async () => unwrap(await engine.getEntity(vault, id)) });
};

const MergePreviewTable = ({ source, target }: { readonly source: EntityDetail; readonly target: EntityDetail }) => {
  const preview = useMemo(() => mergePreview(source, target), [source, target]);
  return (
    <section aria-label="Merge preview" data-merge-preview={true} className="flex flex-col gap-2 rounded-lg border border-border-strong bg-surface-raised p-3">
      <h3 className="font-medium">What this merge would do</h3>
      {!preview.sameType && <Callout tone="danger" title={`These are different types: ${source.typeName} and ${target.typeName}.`}>The server decides whether that can be merged.</Callout>}
      <table className="w-full text-sm">
        <thead>
          <tr className="border-b border-border-strong text-left text-xs text-text-muted">
            <th className="p-1.5 font-medium">Field</th><th className="p-1.5 font-medium">This entity</th><th className="p-1.5 font-medium">The one that stays</th><th className="p-1.5 font-medium">After the merge</th>
          </tr>
        </thead>
        <tbody>
          {preview.rows.map((r) => (
            <tr key={r.field} data-merge-row={r.field} data-merge-outcome={r.outcome} className="border-b border-border align-top">
              <td className="p-1.5 font-mono text-xs">{r.field}</td>
              <td className="p-1.5 break-words">{r.sourceValue ?? <span className="text-text-muted">No value</span>}</td>
              <td className="p-1.5 break-words">{r.targetValue ?? <span className="text-text-muted">No value</span>}</td>
              <td className="p-1.5 text-xs">{r.outcome === "conflict" ? <Badge tone="warning">Conflict</Badge> : null} {MERGE_OUTCOME_TEXT[r.outcome]}</td>
            </tr>
          ))}
        </tbody>
      </table>
      <p className="text-sm text-text-muted">Aliases afterwards: <span className="text-text">{preview.aliases.length === 0 ? "none" : preview.aliases.join(", ")}</span></p>
      <p className="text-sm text-text-muted">This entity's address would lead to the one that stays, for as long as the merge stands. A merge can be reverted, and it always waits for a person to approve it.</p>
    </section>
  );
};

/** Choosing the other entity: a suggestion can name it, or a search finds it among the same type. */
const TargetPicker = ({ vault, source, into, onPick }: { readonly vault: VaultId; readonly source: EntityDetail; readonly into: string; readonly onPick: (id: string) => void }) => {
  const engine = useEngine();
  const [mention, setMention] = useState("");
  const [asked, setAsked] = useState("");
  const { view } = useEntityType(vault, source.typeName);
  const found = useQuery({ queryKey: ENTITY_KEYS.resolve(vault, source.typeName, asked), enabled: asked !== "", queryFn: async () => unwrap(await engine.resolveEntities(vault, asked, source.typeName)) });
  const others = found.data?.candidates.filter((c) => c.entity.id !== source.id) ?? [];
  return (
    <div className="flex flex-col gap-2">
      <form aria-label="Find the entity to merge into" className="flex items-end gap-2" onSubmit={(e) => { e.preventDefault(); setAsked(mention.trim()); }}>
        <div className="w-72"><Input label="Find the entity to merge into" value={mention} onChange={(e) => setMention(e.target.value)} /></div>
        <Button type="submit">Find</Button>
      </form>
      {asked !== "" && found.data !== undefined && others.length === 0 && <p className="text-sm text-text-muted">No other {source.typeName} matches that.</p>}
      {others.length > 0 && (
        <ul className="flex flex-col gap-1 text-sm">
          {others.map((c) => (
            <li key={c.entity.id} className="flex items-center gap-2" data-merge-candidate={c.entity.id}>
              <Button aria-pressed={into === c.entity.id} onClick={() => onPick(c.entity.id)}>Choose</Button>
              <span className="break-words">{entityLabel(c.entity.attributes, c.entity.aliases, view?.fields ?? []) ?? c.entity.id}</span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
};

export const MergeEntity = ({ vault, id, into }: { readonly vault: VaultId; readonly id: string; readonly into: string }) => {
  const engine = useEngine();
  const go = useGo();
  const refresh = useRefreshEntities(vault);
  const source = useEntity(vault, id);
  const target = useEntity(vault, into);
  const [note, setNote] = useState("");
  const [outcome, setOutcome] = useState<EntityProposal | null>(null);
  const submit = useMutation({
    mutationFn: () => engine.mergeEntity(vault, id, { into, rationale: note.trim() === "" ? DEFAULT_NOTE : note }),
    onSuccess: async (result) => {
      setOutcome(result);
      await refresh();
    },
  });
  return (
    <section aria-label="Propose a merge" className="flex flex-col gap-3">
      <div className="flex items-center justify-between">
        <h2 className="text-lg font-medium">Propose a merge</h2>
        <AppLink to={entityPath(vault, { kind: "entity", id })}>Back to the entity</AppLink>
      </div>
      <QueryView query={source} label="this entity">
        {(from) => (
          <>
            <TargetPicker vault={vault} source={from} into={into} onPick={(next) => go(entityPath(vault, { kind: "merge", id, into: next }), true)} />
            {into !== "" && into === from.id && <Callout tone="warning" title="An entity can't be merged into itself." />}
            {into !== "" && into !== from.id && (
              <QueryView query={target} label="the entity to merge into">
                {(to) => (
                  <>
                    <MergePreviewTable source={from} target={to} />
                    <Input label="Why" value={note} onChange={(e) => setNote(e.target.value)} />
                    {outcome !== null && <ProposalNotice vault={vault} outcome={outcome} what="This merge" />}
                    <div><Button variant="primary" disabled={submit.isPending || outcome?.kind === "pending"} onClick={() => submit.mutate()}>Send this merge to review</Button></div>
                  </>
                )}
              </QueryView>
            )}
          </>
        )}
      </QueryView>
    </section>
  );
};

const toggled = (set: ReadonlySet<string>, id: string, on: boolean): ReadonlySet<string> => {
  const next = new Set(set);
  if (on) next.add(id);
  else next.delete(id);
  return next;
};

const SplitForm = ({ vault, entity }: { readonly vault: VaultId; readonly entity: EntityDetail }) => {
  const engine = useEngine();
  const refresh = useRefreshEntities(vault);
  const { view } = useEntityType(vault, entity.typeName);
  const [moved, setMoved] = useState<ReadonlySet<string>>(new Set());
  const [aliases, setAliases] = useState<ReadonlySet<string>>(new Set());
  const [values, setValues] = useState<Readonly<Record<string, ValueDraft>>>({});
  const [note, setNote] = useState("");
  const [problems, setProblems] = useState<readonly string[]>([]);
  const [outcome, setOutcome] = useState<EntityProposal | null>(null);
  const preview = useMemo(() => splitPreview(entity, moved, aliases), [aliases, entity, moved]);
  const submit = useMutation({
    mutationFn: (fields: readonly EntityFieldInput[]) => engine.splitEntity(vault, entity.id, { baseVersion: entity.version, moved: [...moved], aliases: [...aliases], fields, rationale: note.trim() === "" ? DEFAULT_NOTE : note }),
    onSuccess: async (result) => {
      setOutcome(result);
      await refresh();
    },
  });
  const propose = () => {
    const wrong: string[] = moved.size === 0 && aliases.size === 0 ? ["Choose at least one claim or alias to move."] : [];
    const fields: EntityFieldInput[] = [];
    for (const f of view?.fields ?? []) {
      const draft = values[f.name] ?? EMPTY_VALUE;
      const { evidence, problem } = evidenceOf(draft);
      if (problem !== null) wrong.push(`${f.name}: ${problem}`);
      if (draft.raw.trim() === "") continue;
      const parsed = parseFieldValue(f, draft.raw);
      if (parsed.ok) fields.push({ field: f.name, value: parsed.value, evidence, replaces: null });
      else wrong.push(parsed.problem);
    }
    setProblems(wrong);
    if (wrong.length === 0) submit.mutate(fields);
  };
  const claims = entity.fields.flatMap((f) => [f.current, ...f.conflicts, ...f.agreeing].map((c) => ({ field: f.field, claim: c })));
  return (
    <form className="flex flex-col gap-3" aria-label="Split" onSubmit={(e) => e.preventDefault()}>
      <fieldset className="flex flex-col gap-1 rounded-md border border-border p-2">
        <legend className="px-1 text-xs text-text-muted">Claims that move to the new entity</legend>
        {claims.map(({ field, claim }) => (
          <label key={claim.assertionId} className="flex items-center gap-2 text-sm" data-split-claim={claim.assertionId}>
            <input type="checkbox" checked={moved.has(claim.assertionId)} onChange={(e) => setMoved(toggled(moved, claim.assertionId, e.target.checked))} />
            <span className="font-mono text-xs">{field}</span> <span className="break-words">{valueText(claim.value)}</span>
          </label>
        ))}
      </fieldset>
      {entity.aliases.length > 0 && (
        <fieldset className="flex flex-col gap-1 rounded-md border border-border p-2">
          <legend className="px-1 text-xs text-text-muted">Aliases that move</legend>
          {entity.aliases.map((a) => (
            <label key={a} className="flex items-center gap-2 text-sm">
              <input type="checkbox" checked={aliases.has(a)} onChange={(e) => setAliases(toggled(aliases, a, e.target.checked))} /> <span className="break-words">{a}</span>
            </label>
          ))}
        </fieldset>
      )}
      <details className="text-sm" open={true}>
        <summary className="cursor-pointer text-xs text-text-muted">Anything else the new entity needs to be valid, such as its own name</summary>
        <div className="mt-2 grid grid-cols-1 gap-2 lg:grid-cols-2">
          {view?.fields.map((f) => <ValueInput key={f.name} field={f} idPrefix="split" value={values[f.name] ?? EMPTY_VALUE} onChange={(next) => setValues({ ...values, [f.name]: next })} />)}
        </div>
      </details>
      <section aria-label="Split preview" data-split-preview={true} className="grid grid-cols-2 gap-3 rounded-lg border border-border-strong bg-surface-raised p-3 text-sm">
        <div>
          <h3 className="font-medium">Stays here</h3>
          <ul className="text-xs text-text-muted">
            {preview.staying.map((c, i) => <li key={`${c.field}-${String(i)}`} className="break-words">{c.field}: {c.value}</li>)}
            {preview.stayingAliases.map((a) => <li key={a} className="break-words">alias: {a}</li>)}
          </ul>
        </div>
        <div>
          <h3 className="font-medium">Moves to the new entity</h3>
          <ul className="text-xs text-text-muted" data-split-moved={preview.moved.length + preview.movedAliases.length}>
            {preview.moved.map((c, i) => <li key={`${c.field}-${String(i)}`} className="break-words">{c.field}: {c.value}</li>)}
            {preview.movedAliases.map((a) => <li key={a} className="break-words">alias: {a}</li>)}
          </ul>
        </div>
        <p className="col-span-2 text-xs text-text-muted">A split always waits for a person to approve it, and it can be reverted.</p>
      </section>
      <Input label="Why" value={note} onChange={(e) => setNote(e.target.value)} />
      {problems.length > 0 && <Callout tone="warning" title="Fix these first."><ul className="list-disc pl-4">{problems.map((p) => <li key={p}>{p}</li>)}</ul></Callout>}
      {outcome !== null && <ProposalNotice vault={vault} outcome={outcome} what="This split" />}
      <div><Button variant="primary" disabled={submit.isPending || outcome?.kind === "pending"} onClick={propose}>Send this split to review</Button></div>
    </form>
  );
};

export const SplitEntity = ({ vault, id }: { readonly vault: VaultId; readonly id: string }) => {
  const query = useEntity(vault, id);
  return (
    <section aria-label="Propose a split" className="flex flex-col gap-3">
      <div className="flex items-center justify-between">
        <h2 className="text-lg font-medium">Propose a split</h2>
        <AppLink to={entityPath(vault, { kind: "entity", id })}>Back to the entity</AppLink>
      </div>
      <QueryView query={query} label="this entity">{(entity) => <SplitForm key={entity.version} vault={vault} entity={entity} />}</QueryView>
    </section>
  );
};

/** Suggestions only: nothing merges from here without a proposal a person approves. */
export const SameAsList = ({ vault, type, writable }: { readonly vault: VaultId; readonly type: string; readonly writable: boolean }) => {
  const engine = useEngine();
  const query = useQuery({ queryKey: ENTITY_KEYS.sameAs(vault, type), queryFn: async () => unwrap(await engine.listSameAs(vault, type)) });
  return (
    <section aria-label="Same-as suggestions" className="flex flex-col gap-3">
      <div className="flex items-center justify-between">
        <h2 className="text-lg font-medium break-words">Same-as suggestions for {type}</h2>
        <AppLink to={typeLink(vault, type)}>Back to {type}</AppLink>
      </div>
      <p className="text-sm text-text-muted">These entities share an identity value or an alias. That's a reason to look, not a decision: a merge is a proposal that a person approves.</p>
      <QueryView query={query} label="suggestions" isEmpty={(items) => items.length === 0} empty={<EmptyState title="No entities of this type share an identity value or alias." />}>
        {(items) => (
          <ul className="flex flex-col gap-2">
            {items.map((s) => {
              const [keep, ...rest] = s.entityIds;
              return (
                <li key={`${s.field}-${s.value}`} data-same-as={s.value} className="flex flex-col gap-1 rounded-md border border-border p-2 text-sm">
                  <span>Same <span className="font-mono text-xs">{s.field === "$alias" ? "alias" : s.field}</span>: <span className="font-medium break-words">{s.value}</span></span>
                  <ul className="flex flex-col gap-1">
                    {s.entityIds.map((id) => <li key={id}><AppLink to={entityPath(vault, { kind: "entity", id })}><span className="font-mono text-xs break-all">{id}</span></AppLink></li>)}
                  </ul>
                  {writable && keep !== undefined && (
                    <span className="flex flex-wrap gap-2">
                      {rest.map((id) => <AppLink key={id} to={entityPath(vault, { kind: "merge", id, into: keep })} className={LINK_BUTTON}>Preview merging {id.slice(0, 8)} into {keep.slice(0, 8)}</AppLink>)}
                    </span>
                  )}
                </li>
              );
            })}
          </ul>
        )}
      </QueryView>
    </section>
  );
};
