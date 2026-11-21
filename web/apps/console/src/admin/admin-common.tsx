import { confirmationMatches } from "@memex/core";
import type { ApiFailure } from "@memex/core";
import { Button, Input } from "@memex/ui";
import { Copy } from "lucide-react";
import { useState } from "react";
import type { ReactNode } from "react";
import { Callout, failureText } from "../wiki/wiki-common";

export { Callout, failureText };

export const formatWhen = (ms: number | null): string => (ms === null ? "never" : `${new Date(ms).toISOString().slice(0, 16).replace("T", " ")} UTC`);

/** A section of an admin screen, with its heading. */
export const Panel = ({ title, children, description }: { readonly title: string; readonly children: ReactNode; readonly description?: string }) => (
  <section aria-label={title} className="flex flex-col gap-3 rounded-lg border border-border p-4">
    <h2 className="font-medium">{title}</h2>
    {description !== undefined && <p className="text-sm text-text-muted">{description}</p>}
    {children}
  </section>
);

interface ConfirmProps {
  /** What the person is about to do, as a heading. */
  readonly title: string;
  readonly description: ReactNode;
  /** When set, the action runs only once this has been typed exactly. */
  readonly typed?: { readonly expected: string; readonly label: string };
  readonly confirmLabel: string;
  readonly danger?: boolean;
  readonly run: () => Promise<ApiFailure | null>;
  readonly onDone: () => void;
  readonly onCancel: () => void;
  readonly children?: ReactNode;
}

/**
 * The second step of every action that changes or removes something, in the page rather than a modal, as Radix's modal logs a CSP error
 * per opening (spec 040). Nothing is sent until its button is pressed, and with `typed`, not until the name has been typed too.
 */
export const ConfirmPanel = ({ title, description, typed, confirmLabel, danger = true, run, onDone, onCancel, children }: ConfirmProps) => {
  const [text, setText] = useState("");
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const ready = typed === undefined || confirmationMatches(typed.expected, text);
  const go = async () => {
    if (!ready) return;
    setBusy(true);
    setProblem(null);
    const failure = await run();
    setBusy(false);
    if (failure === null) onDone();
    else setProblem(failureText(failure));
  };
  return (
    <section aria-label={title} data-testid="confirm-panel" className={`flex flex-col gap-2 rounded-lg border p-3 ${danger ? "border-danger bg-danger-surface" : "border-border-strong bg-surface-raised"}`}>
      <h3 className="font-medium">{title}</h3>
      <div className="text-sm text-text-muted">{description}</div>
      {children}
      {typed !== undefined && (
        <Input
          label={typed.label}
          value={text}
          autoComplete="off"
          spellCheck={false}
          onChange={(e) => setText(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") void go();
          }}
        />
      )}
      {problem !== null && <Callout tone="danger" title={problem} />}
      <div className="flex gap-2">
        <Button variant={danger ? "danger" : "primary"} disabled={busy || !ready} onClick={() => void go()}>{confirmLabel}</Button>
        <Button variant="ghost" onClick={onCancel} disabled={busy}>Cancel</Button>
      </div>
    </section>
  );
};

/** Copies a value the page holds. The browser asks nothing for this on a same-origin page that the person clicked on. */
export const CopyButton = ({ text, label }: { readonly text: string; readonly label: string }) => {
  const [state, setState] = useState<"idle" | "copied" | "failed">("idle");
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(text);
      setState("copied");
    } catch {
      setState("failed");
    }
  };
  return (
    <span className="inline-flex items-center gap-2">
      <Button onClick={() => void copy()} aria-label={label}><Copy size={14} aria-hidden={true} /> Copy</Button>
      <span role="status" className="text-xs text-text-muted">{state === "copied" ? "Copied." : state === "failed" ? "The browser didn't allow copying. Select the text and copy it instead." : ""}</span>
    </span>
  );
};

export const CodeBlock = ({ text, label }: { readonly text: string; readonly label: string }) => (
  <pre aria-label={label} className="overflow-x-auto rounded-md border border-border bg-surface-raised p-3 font-mono text-xs text-text">{text}</pre>
);

export const KeyValues = ({ rows, label }: { readonly rows: readonly (readonly [string, ReactNode])[]; readonly label: string }) => (
  <dl aria-label={label} className="grid grid-cols-[max-content_1fr] gap-x-4 gap-y-1 text-sm">
    {rows.map(([k, v]) => (
      <div key={k} className="contents">
        <dt className="text-text-muted">{k}</dt>
        <dd className="min-w-0 break-words text-text">{v}</dd>
      </div>
    ))}
  </dl>
);
