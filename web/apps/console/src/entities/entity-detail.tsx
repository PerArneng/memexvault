import { documentPath, entityLabel, entityPath, sectionPath, valueText } from "@memex/core";
import type { EntityDetail, EntityFieldDetail, FieldClaim, FieldEvidence, VaultId } from "@memex/types";
import { Badge, ConflictBadge } from "@memex/ui";
import { useQuery } from "@tanstack/react-query";
import { Bot, UserRound } from "lucide-react";
import { useEngine } from "../engine-context";
import { unwrap } from "../query";
import { QueryView } from "../shell/query-view";
import { EpistemicOf, TierOf } from "../viewer/badges";
import { sourceRoute } from "../documents/document-common";
import { AppLink, Callout, formatTime, LINK_BUTTON } from "../wiki/wiki-common";
import { ENTITY_KEYS, typeLink, useEntityType } from "./entity-common";
import { GraphPanel } from "./graph-panel";

const pagesText = (e: FieldEvidence): string | null => {
  if (e.pageFrom === null) return null;
  const [from, to] = [e.pageFrom + 1, (e.pageTo ?? e.pageFrom) + 1];
  return from === to ? `page ${String(from)}` : `pages ${String(from)} to ${String(to)}`;
};

/** One piece of evidence, and the way to the highlighted place in spec 042's viewer. A broken anchor links to lineage instead, as a highlight there would be a guess. */
const EvidenceRow = ({ vault, claim, evidence }: { readonly vault: VaultId; readonly claim: string; readonly evidence: FieldEvidence }) => {
  const lineage = documentPath(vault, { kind: "lineage", claim, evidence: evidence.evidenceId });
  const pages = pagesText(evidence);
  return (
    <li data-evidence-row={evidence.evidenceId} className="flex flex-col gap-1 text-sm">
      <q className="whitespace-pre-wrap break-words">{evidence.quote}</q>
      <span className="flex flex-wrap items-center gap-2 text-xs text-text-muted">
        <Badge tone={evidence.stance === "supports" ? "success" : "warning"}>{evidence.stance}</Badge>
        {evidence.trustTier !== null && <TierOf tier={evidence.trustTier} />}
        {evidence.sourceName !== null && <span className="break-all">{evidence.sourceName}</span>}
        {pages !== null && <span>{pages}</span>}
        {evidence.anchorBroken && <Badge tone="danger">Anchor broken</Badge>}
        {evidence.sourceId !== null && !evidence.anchorBroken && <AppLink to={documentPath(vault, sourceRoute(evidence.sourceId, { claim, evidence: evidence.evidenceId }))} data-link-state="evidence">Show it in the source</AppLink>}
        {evidence.sourceId === null && <span>The source isn't available.</span>}
        <AppLink to={lineage}>Lineage</AppLink>
      </span>
    </li>
  );
};

const ClaimFacts = ({ vault, claim }: { readonly vault: VaultId; readonly claim: FieldClaim }) => (
  <span className="flex flex-wrap items-center gap-2 text-xs text-text-muted">
    <EpistemicOf status={claim.epistemic} />
    {claim.machine === true && <Badge tone="info"><Bot size={12} aria-hidden={true} /> Agent</Badge>}
    {claim.machine === false && <Badge><UserRound size={12} aria-hidden={true} /> Person</Badge>}
    {claim.creator !== null && <span className="break-all">{claim.creator}</span>}
    {claim.changesetId === null && <span>This claim can't be read any more.</span>}
    <AppLink to={documentPath(vault, { kind: "lineage", claim: claim.assertionId, evidence: null })}>Lineage</AppLink>
  </span>
);

const EvidenceList = ({ vault, claim }: { readonly vault: VaultId; readonly claim: FieldClaim }) =>
  claim.evidence.length === 0 ? (
    <p className="text-xs text-text-muted" data-no-evidence={true}>No evidence: {claim.machine === false ? "a person stated this." : "none can be followed."}</p>
  ) : (
    <ul className="flex flex-col gap-2">
      {claim.evidence.map((e) => <EvidenceRow key={e.evidenceId} vault={vault} claim={claim.assertionId} evidence={e} />)}
    </ul>
  );

const OtherClaim = ({ vault, claim, role }: { readonly vault: VaultId; readonly claim: FieldClaim; readonly role: "conflict" | "agreeing" }) => (
  <li data-claim-role={role} data-claim={claim.assertionId} className="flex flex-col gap-1 rounded-md border border-border p-2">
    <span className="flex flex-wrap items-center gap-2">
      <span data-claim-value={true} className="font-medium break-words whitespace-pre-wrap">{valueText(claim.value)}</span>
      {role === "conflict" && <Badge tone="warning">Not chosen</Badge>}
    </span>
    <ClaimFacts vault={vault} claim={claim} />
    <EvidenceList vault={vault} claim={claim} />
  </li>
);

/** A field's current value, why it's the one, and every claim that disagrees, side by side. Values are text nodes: nothing here can become markup. */
const FieldCard = ({ vault, field }: { readonly vault: VaultId; readonly field: EntityFieldDetail }) => (
  <section aria-label={`Field ${field.field}`} data-field={field.field} className="flex flex-col gap-2 rounded-lg border border-border p-3">
    <h3 className="flex flex-wrap items-center gap-2 text-xs font-medium text-text-muted"><span className="font-mono">{field.field}</span>{field.conflicts.length > 0 && <ConflictBadge count={field.conflicts.length} />}</h3>
    <div data-claim-role="current" data-claim={field.current.assertionId} className="flex flex-col gap-1">
      <span className="flex flex-wrap items-center gap-2">
        <span data-claim-value={true} className="text-base break-words whitespace-pre-wrap">{valueText(field.value)}</span>
        {field.conflicts.length > 0 && <Badge tone="success">Current choice</Badge>}
      </span>
      <ClaimFacts vault={vault} claim={field.current} />
      <p className="text-xs text-text-muted" data-field-why={true}>Why this one: {field.why}</p>
      <EvidenceList vault={vault} claim={field.current} />
    </div>
    {field.conflicts.length > 0 && (
      <div className="flex flex-col gap-1">
        <h4 className="text-xs font-medium text-warning">Other sources disagree</h4>
        <ul className="flex flex-col gap-2">{field.conflicts.map((c) => <OtherClaim key={c.assertionId} vault={vault} claim={c} role="conflict" />)}</ul>
      </div>
    )}
    {field.agreeing.length > 0 && (
      <details className="text-sm">
        <summary className="cursor-pointer text-xs text-text-muted">{field.agreeing.length} more {field.agreeing.length === 1 ? "claim agrees" : "claims agree"}</summary>
        <ul className="mt-1 flex flex-col gap-2">{field.agreeing.map((c) => <OtherClaim key={c.assertionId} vault={vault} claim={c} role="agreeing" />)}</ul>
      </details>
    )}
  </section>
);

const RedirectNotice = ({ vault, asked, id }: { readonly vault: VaultId; readonly asked: string; readonly id: string }) => {
  const engine = useEngine();
  const query = useQuery({ queryKey: ENTITY_KEYS.redirect(vault, asked), queryFn: async () => unwrap(await engine.getEntityRedirect(vault, asked)) });
  return (
    <div data-redirected-from={asked}>
      <Callout tone="info" title="This entity was merged, so its old address leads here.">
        <span className="font-mono text-xs break-all">{asked}</span> now leads to <span className="font-mono text-xs break-all">{id}</span>
        {query.data !== undefined && query.data.chain.length > 1 && <>, through {query.data.chain.length} steps</>}.
      </Callout>
    </div>
  );
};

const Loaded = ({ vault, entity, writable }: { readonly vault: VaultId; readonly entity: EntityDetail; readonly writable: boolean }) => {
  const { view } = useEntityType(vault, entity.typeName);
  const attributes = Object.fromEntries(entity.fields.map((f) => [f.field, f.value]));
  const label = entityLabel(attributes, entity.aliases, view?.fields ?? []) ?? entity.id;
  return (
    <section aria-label="Entity" data-entity={entity.id} className="flex flex-col gap-3">
      <div className="flex flex-wrap items-start justify-between gap-2">
        <div className="flex min-w-0 flex-col gap-1">
          <h2 className="text-lg font-medium break-words">{label}</h2>
          <p className="flex flex-wrap items-center gap-2 text-xs text-text-muted">
            <AppLink to={typeLink(vault, entity.typeName)}>{entity.typeName}</AppLink>
            <span>type version {entity.typeVersion}</span>
            <span>revision {entity.version}</span>
            <span className="font-mono">{formatTime(entity.updatedAt)}</span>
            <span className="font-mono break-all">{entity.id}</span>
          </p>
        </div>
        {writable && (
          <span className="flex flex-wrap gap-2">
            <AppLink to={entityPath(vault, { kind: "edit", id: entity.id })} className={LINK_BUTTON}>Propose an edit</AppLink>
            <AppLink to={entityPath(vault, { kind: "merge", id: entity.id, into: "" })} className={LINK_BUTTON}>Propose a merge</AppLink>
            <AppLink to={entityPath(vault, { kind: "split", id: entity.id })} className={LINK_BUTTON}>Propose a split</AppLink>
          </span>
        )}
      </div>
      {entity.redirectedFrom !== null && <RedirectNotice vault={vault} asked={entity.redirectedFrom} id={entity.id} />}
      <dl className="grid grid-cols-[7rem_minmax(0,1fr)] gap-x-2 gap-y-1 text-sm">
        <dt className="text-xs text-text-muted">Aliases</dt>
        <dd data-aliases={true} className="break-words">{entity.aliases.length === 0 ? <span className="text-text-muted">None</span> : entity.aliases.join(", ")}</dd>
        {entity.mergedFrom.length > 0 && (
          <>
            <dt className="text-xs text-text-muted">Merged from</dt>
            <dd className="flex flex-wrap gap-2 font-mono text-xs">{entity.mergedFrom.map((m) => <span key={m} className="break-all">{m}</span>)}</dd>
          </>
        )}
        <dt className="text-xs text-text-muted">Last change</dt>
        <dd><AppLink to={sectionPath(vault, "review", entity.changesetId)}>{entity.changesetId}</AppLink></dd>
      </dl>
      {entity.fields.length === 0 ? <p className="text-sm text-text-muted">This entity has no values yet.</p> : <div className="flex flex-col gap-2">{entity.fields.map((f) => <FieldCard key={f.field} vault={vault} field={f} />)}</div>}
      <GraphPanel vault={vault} entity={entity} label={label} />
    </section>
  );
};

export const EntityDetailView = ({ vault, id, writable }: { readonly vault: VaultId; readonly id: string; readonly writable: boolean }) => {
  const engine = useEngine();
  const query = useQuery({ queryKey: ENTITY_KEYS.entity(vault, id), queryFn: async () => unwrap(await engine.getEntity(vault, id)) });
  return <QueryView query={query} label="this entity">{(entity) => <Loaded vault={vault} entity={entity} writable={writable} />}</QueryView>;
};
