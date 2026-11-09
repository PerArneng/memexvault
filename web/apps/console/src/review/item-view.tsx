import { automaticPath, describeOp, percent, validatorLine } from "@memex/core";
import type { AutomaticPath } from "@memex/core";
import type { ChangeSet, ReviewDetail, ReviewVerdict } from "@memex/types";
import { Badge } from "@memex/ui";
import type { Tone } from "@memex/ui";
import { formatTime } from "../wiki/wiki-common";
import { EpistemicOf, StateBadge, TrustOf, WhoBadge } from "./review-common";

export const OpsList = ({ ops, label }: { readonly ops: readonly unknown[]; readonly label: string }) => (
  <section aria-label={label} className="flex flex-col gap-2">
    <h2 className="text-sm font-medium">{label}</h2>
    {ops.map((op, i) => {
      const summary = describeOp(op);
      return (
        // Ops have no ids of their own, and this list never reorders.
        <div key={i} data-testid="op" className="flex flex-col gap-1 rounded-md border border-border p-2 text-sm">
          <div className="flex flex-wrap items-center gap-2">
            <span className="font-medium">{summary.title}</span>
            <EpistemicOf status={summary.epistemic} />
          </div>
          <dl className="grid grid-cols-[6rem_minmax(0,1fr)] gap-x-2 gap-y-0.5">
            {summary.fields.map((f) => (
              <div key={f.label} className="contents">
                <dt className="text-text-muted">{f.label}</dt>
                <dd className="break-words whitespace-pre-wrap">{f.value}</dd>
              </div>
            ))}
          </dl>
          <details className="text-xs text-text-muted">
            <summary>The op as JSON</summary>
            <pre className="overflow-auto font-mono whitespace-pre-wrap">{JSON.stringify(op, null, 2)}</pre>
          </details>
        </div>
      );
    })}
  </section>
);

const PATH_TONE: Readonly<Record<AutomaticPath["tone"], Tone>> = { never: "warning", off: "neutral", on: "info" };

export const PathNotice = ({ path }: { readonly path: AutomaticPath }) => (
  <div data-testid="automatic-path" data-path={path.tone} className="flex flex-col gap-1 rounded-md border border-border p-2 text-sm">
    <span className="flex items-center gap-2"><Badge tone={PATH_TONE[path.tone]}>Auto-commit</Badge><span className="font-medium">{path.title}</span></span>
    <span className="text-text-muted">{path.detail}</span>
  </div>
);

const VerdictView = ({ verdict }: { readonly verdict: ReviewVerdict | null }) => {
  if (verdict === null) return <p className="text-sm text-text-muted">The reviewer agent hasn't given a verdict on this.</p>;
  const tone: Tone = verdict.verdict === "approve" ? "success" : verdict.verdict === "reject" ? "danger" : "neutral";
  return (
    <div data-testid="verdict" className="flex flex-col gap-1 text-sm">
      <span className="flex flex-wrap items-center gap-2"><Badge tone={tone}>{verdict.verdict}</Badge><span className="text-xs text-text-muted">{verdict.model}. It's advice: it decides nothing and never counts as a sample.</span></span>
      <p className="break-words whitespace-pre-wrap">{verdict.reason}</p>
    </div>
  );
};

export const ValidatorsView = ({ validators }: { readonly validators: readonly unknown[] }) => {
  if (validators.length === 0) return <p className="text-sm text-text-muted">No validator results on record.</p>;
  return (
    <ul data-testid="validators" className="flex flex-col gap-1 text-sm">
      {validators.map((raw, i) => {
        const line = validatorLine(raw);
        return (
          <li key={i} className="flex flex-wrap items-center gap-2">
            <Badge tone={line.passed ? "success" : "danger"}>{line.passed ? "Passed" : "Failed"}</Badge>
            <span>{line.name}</span>
            {line.detail !== "" && <span className="min-w-0 break-words text-text-muted">{line.detail}</span>}
          </li>
        );
      })}
    </ul>
  );
};

export const DecisionFacts = ({ changeset }: { readonly changeset: ChangeSet }) =>
  changeset.decidedBy === null ? null : (
    <dl data-testid="decision" className="grid grid-cols-[6rem_minmax(0,1fr)] gap-x-2 text-sm">
      <dt className="text-text-muted">Decided by</dt>
      <dd data-testid="decided-by">{changeset.decidedBy}</dd>
      {changeset.decidedAt !== null && <><dt className="text-text-muted">When</dt><dd>{formatTime(changeset.decidedAt)}</dd></>}
      {/* A revert's reason is its rationale, which is shown just below. */}
      {(changeset.decisionReason !== null || changeset.reverts === null) && <><dt className="text-text-muted">Reason</dt>
      <dd data-testid="decision-reason" className="break-words whitespace-pre-wrap">{changeset.decisionReason ?? "None given"}</dd></>}
    </dl>
  );

export const ChangeHeader = ({ changeset }: { readonly changeset: ChangeSet }) => (
  <div className="flex flex-wrap items-center gap-2 text-sm">
    <StateBadge state={changeset.state} />
    <WhoBadge machine={changeset.proposerIsMachine} />
    <span data-testid="proposer" className="min-w-0 truncate">{changeset.proposer}</span>
    {changeset.extractor !== null && changeset.extractor !== changeset.proposer && <span className="text-text-muted">via {changeset.extractor}</span>}
    {changeset.requestedBy !== null && <span className="text-text-muted">for {changeset.requestedBy}, in a chat</span>}
    <time className="font-mono text-xs text-text-muted tabular-nums">{formatTime(changeset.proposedAt)}</time>
  </div>
);

export const Rationale = ({ text }: { readonly text: string }) => (
  <section aria-label="Rationale" className="flex flex-col gap-1">
    <h2 className="text-sm font-medium">Rationale</h2>
    <p data-testid="rationale" className="text-sm break-words whitespace-pre-wrap">{text === "" ? "None given" : text}</p>
  </section>
);

/** The proposal, what has been said about it, and what could happen to it. All of its text is untrusted and goes in as text. */
export const ItemView = ({ detail }: { readonly detail: ReviewDetail }) => (
  <div className="flex min-w-0 flex-col gap-3" data-testid="item" data-item={detail.changeset.id}>
    <ChangeHeader changeset={detail.changeset} />
    <PathNotice path={automaticPath(detail)} />
    <DecisionFacts changeset={detail.changeset} />
    <Rationale text={detail.changeset.rationale} />
    <OpsList ops={detail.changeset.ops} label="What it does" />
    {detail.changeset.originalOps !== null && <OpsList ops={detail.changeset.originalOps} label="What was first proposed" />}
    <section aria-label="Reviewer agent" className="flex flex-col gap-1">
      <h2 className="text-sm font-medium">Reviewer agent</h2>
      <VerdictView verdict={detail.verdict} />
    </section>
    <section aria-label="Validators" className="flex flex-col gap-1">
      <h2 className="text-sm font-medium">Validators</h2>
      <ValidatorsView validators={detail.changeset.validators} />
    </section>
    {detail.combinations.length > 0 && (
      <section aria-label="Its combination" className="flex flex-col gap-1">
        <h2 className="text-sm font-medium">Its combination</h2>
        <ul className="flex flex-col gap-1 text-sm">
          {detail.combinations.map((c) => (
            <li key={c.id} className="flex flex-wrap items-center gap-2">
              <Badge tone={c.on ? "info" : "neutral"}>{c.on ? "On" : "Off"}</Badge>
              <span>{c.itemType}</span>
              <span className="text-text-muted">{c.correct} correct, {c.incorrect} wrong, lower bound {percent(c.wilsonBound)}</span>
            </li>
          ))}
        </ul>
      </section>
    )}
    {detail.evidence.some((e) => e.trustTier !== "trusted" && e.trustTier !== "verified") && (
      <p className="flex flex-wrap items-center gap-2 text-sm text-text-muted"><TrustOf tier={detail.evidence.find((e) => e.trustTier !== "trusted" && e.trustTier !== "verified")?.trustTier ?? null} /> It cites a source no person has marked trusted.</p>
    )}
  </div>
);
