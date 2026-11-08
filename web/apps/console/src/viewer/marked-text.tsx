import type { MarkedPart } from "@memex/core";

/** Text with some of it marked. Each part stays plain text, so vault content can't become markup. */
export const MarkedText = ({ parts }: { readonly parts: readonly MarkedPart[] }) => (
  <>
    {parts.map((part, i) =>
      part.marked ? (
        <mark key={i} className="rounded-sm bg-warning-surface text-text underline decoration-warning underline-offset-2">{part.text}</mark>
      ) : (
        <span key={i}>{part.text}</span>
      ),
    )}
  </>
);
