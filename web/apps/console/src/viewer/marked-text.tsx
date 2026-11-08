import type { MarkedPart } from "@memex/core";
import { Fragment } from "react";

/** Text with some of it marked. Each part stays plain text, so vault content can't become markup. */
export const MarkedText = ({ parts }: { readonly parts: readonly MarkedPart[] }) => (
  <>
    {parts.map((part, i) =>
      part.marked ? (
        <mark key={i} className="rounded-sm bg-warning-surface text-text underline decoration-warning underline-offset-2">{part.text}</mark>
      ) : (
        // No element for the text between marks, as a page of results can hold thousands of these.
        <Fragment key={i}>{part.text}</Fragment>
      ),
    )}
  </>
);
