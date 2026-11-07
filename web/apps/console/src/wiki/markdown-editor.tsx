import { autocompletion, completionKeymap } from "@codemirror/autocomplete";
import type { Completion, CompletionContext, CompletionResult } from "@codemirror/autocomplete";
import { defaultKeymap, history, historyKeymap } from "@codemirror/commands";
import { markdown } from "@codemirror/lang-markdown";
import { HighlightStyle, syntaxHighlighting } from "@codemirror/language";
import { EditorState } from "@codemirror/state";
import { EditorView, keymap } from "@codemirror/view";
import { completionText, openReferenceAt } from "@memex/core";
import { tags } from "@lezer/highlight";
import { useEffect, useRef } from "react";

export interface EditorOption {
  /** What goes in the text, such as a slug or a claim id. */
  readonly value: string;
  readonly label: string;
  readonly detail: string;
}

export interface MarkdownEditorProps {
  readonly label: string;
  readonly initial: string;
  readonly onChange: (text: string) => void;
  readonly onSave: () => void;
  readonly suggest: (kind: "link" | "claim", query: string) => Promise<readonly EditorOption[]>;
}

const THEME = EditorView.theme(
  {
    "&": { height: "100%", color: "var(--color-text)", backgroundColor: "var(--color-surface-raised)", fontSize: "var(--text-sm)" },
    "&.cm-focused": { outline: "2px solid var(--color-focus)", outlineOffset: "-2px" },
    ".cm-scroller": { fontFamily: "var(--font-mono)", overflow: "auto", lineHeight: "1.5" },
    ".cm-content": { caretColor: "var(--color-text)", padding: "8px" },
    "&.cm-focused .cm-cursor": { borderLeftColor: "var(--color-text)" },
    ".cm-selectionBackground, &.cm-focused .cm-selectionBackground, ::selection": { backgroundColor: "var(--color-surface-selected)" },
    ".cm-tooltip": { border: "1px solid var(--color-border-strong)", backgroundColor: "var(--color-surface-overlay)", color: "var(--color-text)" },
    ".cm-tooltip.cm-tooltip-autocomplete > ul": { fontFamily: "var(--font-sans)", maxHeight: "16rem" },
    ".cm-tooltip-autocomplete ul li[aria-selected]": { backgroundColor: "var(--color-surface-selected)", color: "var(--color-text)" },
    ".cm-completionDetail": { color: "var(--color-text-muted)", marginLeft: "0.75em", fontStyle: "normal" },
  },
  { dark: true },
);

// Syntax colours from the design tokens, so they meet the same contrast checks as everything else.
const HIGHLIGHT = HighlightStyle.define([
  { tag: tags.heading, color: "var(--color-text)", fontWeight: "600" },
  { tag: tags.strong, fontWeight: "600" },
  { tag: tags.emphasis, fontStyle: "italic" },
  { tag: [tags.link, tags.url], color: "var(--color-accent)" },
  { tag: [tags.monospace, tags.quote], color: "var(--color-info)" },
  { tag: [tags.processingInstruction, tags.meta, tags.contentSeparator], color: "var(--color-text-muted)" },
]);

/**
 * CodeMirror writes its styles at runtime. In a document it uses a `<style>` element, which the server's CSP blocks, leaving the editor
 * broken. In a shadow root it uses a constructable stylesheet, which the CSP allows. So it lives in one: see spec 044's Result for the measurements.
 * Design tokens are custom properties, and those cross into a shadow root.
 */
const MarkdownEditor = ({ label, initial, onChange, onSave, suggest }: MarkdownEditorProps) => {
  const host = useRef<HTMLDivElement>(null);
  // The latest callbacks, as the editor is made once.
  const latest = useRef({ onChange, onSave, suggest });
  useEffect(() => {
    latest.current = { onChange, onSave, suggest };
  });

  useEffect(() => {
    const element = host.current;
    if (element === null) return undefined;
    const shadow = element.shadowRoot ?? element.attachShadow({ mode: "open" });
    const source = async (context: CompletionContext): Promise<CompletionResult | null> => {
      const open = openReferenceAt(context.state.doc.toString(), context.pos);
      if (open === null) return null;
      const found = await latest.current.suggest(open.kind, open.query);
      if (context.aborted) return null;
      const options = found.map((option): Completion => ({
        label: option.label,
        detail: option.detail,
        apply: (view, _completion, from, to) => {
          const after = view.state.doc.sliceString(to, to + 2);
          const text = completionText(open.kind, option.value, after);
          const end = from + text.length + (text === option.value ? 2 : 0);
          view.dispatch({ changes: { from, to, insert: text }, selection: { anchor: end } });
        },
      }));
      // The ranking and matching are ours, on what was typed, including spaces.
      return { from: open.from, to: context.pos, options, filter: false };
    };
    const view = new EditorView({
      root: shadow,
      parent: shadow,
      state: EditorState.create({
        doc: initial,
        extensions: [
          history(),
          markdown(),
          EditorView.lineWrapping,
          THEME,
          syntaxHighlighting(HIGHLIGHT),
          autocompletion({ override: [source], activateOnTyping: true, icons: false, defaultKeymap: false }),
          keymap.of([{ key: "Mod-s", preventDefault: true, run: () => { latest.current.onSave(); return true; } }, ...completionKeymap, ...historyKeymap, ...defaultKeymap]),
          // The tab index says out loud what `contenteditable` already gives, as axe doesn't count that as focusable.
          EditorView.contentAttributes.of({ "aria-label": label, tabindex: "0" }),
          EditorView.updateListener.of((update) => {
            if (update.docChanged) latest.current.onChange(update.state.doc.toString());
          }),
        ],
      }),
    });
    return () => view.destroy();
    // Made once per mount: a caller replaces the text by giving this a new `key`.
  }, []);

  return <div ref={host} data-testid="markdown-editor" className="block h-full min-h-0 w-full" />;
};

export default MarkdownEditor;
