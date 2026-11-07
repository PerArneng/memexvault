import { conflictCount, mergedBody, threeWayMerge } from "@memex/core";
import type { MergeChoice, MergeHunk } from "@memex/core";
import type { VaultId, WikiPage } from "@memex/types";
import { Button } from "@memex/ui";
import { useMemo, useState } from "react";
import { Callout, formatTime } from "./wiki-common";

const CHOICES: readonly { readonly value: MergeChoice; readonly label: string }[] = [
  { value: "mine", label: "Keep mine" },
  { value: "theirs", label: "Keep theirs" },
  { value: "both", label: "Keep both" },
];

const Text = ({ label, text }: { readonly label: string; readonly text: string }) => (
  <section aria-label={label} className="flex min-w-0 flex-1 flex-col gap-1">
    <h4 className="text-xs font-medium text-text-muted">{label}</h4>
    <pre tabIndex={0} className="max-h-72 overflow-auto rounded-md border border-border bg-surface-raised p-2 font-mono text-xs break-words whitespace-pre-wrap">{text === "" ? "(empty)" : text}</pre>
  </section>
);

const HunkChoice = ({ hunk, choice, onChoose }: { readonly hunk: Extract<MergeHunk, { kind: "conflict" }>; readonly choice: MergeChoice | undefined; readonly onChoose: (choice: MergeChoice) => void }) => (
  <fieldset className="flex flex-col gap-2 rounded-md border border-warning p-2">
    <legend className="px-1 text-xs font-medium text-warning">Both changed this part</legend>
    <div className="flex gap-2">
      <Text label="Yours" text={hunk.mine.join("\n\n")} />
      <Text label="Theirs" text={hunk.theirs.join("\n\n")} />
    </div>
    <div className="flex gap-4 text-sm">
      {CHOICES.map((c) => (
        <label key={c.value} className="inline-flex items-center gap-1">
          <input type="radio" name={`hunk-${String(hunk.id)}`} checked={choice === c.value} onChange={() => onChoose(c.value)} />
          {c.label}
        </label>
      ))}
    </div>
  </fieldset>
);

export interface ConflictViewProps {
  readonly vault: VaultId;
  readonly mine: string;
  readonly theirs: WikiPage;
  readonly baseBody: string | null;
  readonly baseVersion: number;
  readonly saving: boolean;
  readonly onSaveMerged: (body: string) => void;
  readonly onEditMerged: (body: string) => void;
  readonly onUseTheirs: () => void;
  readonly onBack: () => void;
}

/** Someone else saved first. Nothing has been written, and nothing will be until the person here says what they want. It isn't a modal, so it logs no CSP error. */
export const ConflictView = ({ mine, theirs, baseBody, baseVersion, saving, onSaveMerged, onEditMerged, onUseTheirs, onBack }: ConflictViewProps) => {
  const hunks = useMemo(() => threeWayMerge(baseBody ?? "", mine, theirs.body), [baseBody, mine, theirs.body]);
  const [choices, setChoices] = useState<Readonly<Record<number, MergeChoice>>>({});
  const merged = mergedBody(hunks, choices);
  const count = conflictCount(hunks);
  return (
    <section aria-label="Conflict" data-testid="conflict-view" className="flex flex-col gap-3">
      <h2 className="text-lg font-medium">Editing {theirs.title}: a conflict</h2>
      <Callout tone="warning" title="This page changed while you were editing, and your version hasn't been saved.">
        You started from revision {baseVersion}. It's now at revision {theirs.currentRevision}, last saved by {theirs.creator} at {formatTime(theirs.updatedAt)}. Your text is safe as a draft in this browser.
        {baseBody === null && " The revision you started from can't be read, so every difference counts as a conflict."}
      </Callout>
      <h3 className="font-medium">Both versions</h3>
      <div className="flex gap-3">
        <Text label="Your version" text={mine} />
        <Text label={`Current version, revision ${String(theirs.currentRevision)}`} text={theirs.body} />
      </div>
      <h3 className="font-medium">Merge</h3>
      <p className="text-sm text-text-muted">
        {count === 0 ? "Your changes and theirs don't touch the same parts, so they combine as shown below." : `${String(count)} ${count === 1 ? "part needs" : "parts need"} a choice. Everything else combines by itself.`}
      </p>
      {hunks.flatMap((h) => (h.kind === "conflict" ? [<HunkChoice key={h.id} hunk={h} choice={choices[h.id]} onChoose={(c) => setChoices({ ...choices, [h.id]: c })} />] : []))}
      {merged !== null && <Text label="Merged result" text={merged} />}
      <div className="flex flex-wrap gap-2">
        <Button variant="primary" disabled={merged === null || saving} onClick={() => { if (merged !== null) onSaveMerged(merged); }}>Save merged version</Button>
        <Button disabled={merged === null} onClick={() => { if (merged !== null) onEditMerged(merged); }}>Edit merged version</Button>
        <Button onClick={onUseTheirs}>Use theirs and discard mine</Button>
        <Button variant="ghost" onClick={onBack}>Back to my text</Button>
      </div>
    </section>
  );
};
