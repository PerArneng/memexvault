import { changeSearch, parseAsOf, parseInstant, SEARCH_EPISTEMIC, SEARCH_EXPANDS, SEARCH_KINDS, SEARCH_MODES, toggled, TRUST_TIERS } from "@memex/core";
import type { SearchExpand, SearchMode, SearchState } from "@memex/core";
import type { FacetCount, SearchFacets } from "@memex/types";
import { Input } from "@memex/ui";
import { useEffect, useState } from "react";

const LABELS: Readonly<Record<string, string>> = { claim: "Claims", segment: "Passages", entity: "Entities", page: "Pages", block: "Page sections" };
const CHIP = "inline-flex min-h-6 items-center gap-1 rounded-sm border px-1.5 text-xs break-all";

const Chips = ({ title, values, selected, facets, labelOf, onToggle }: { readonly title: string; readonly values: readonly string[]; readonly selected: readonly string[]; readonly facets: readonly FacetCount[] | undefined; readonly labelOf?: (value: string) => string; readonly onToggle: (value: string) => void }) => {
  const all = [...new Set([...values, ...(facets ?? []).map((f) => f.value), ...selected])];
  if (all.length === 0) return null;
  return (
    <fieldset className="flex flex-col gap-1">
      <legend className="mb-1 text-xs text-text-muted">{title}</legend>
      <div className="flex flex-wrap gap-1">
        {all.map((value) => {
          const count = facets?.find((f) => f.value === value)?.count;
          const on = selected.includes(value);
          return (
            <button key={value} type="button" aria-pressed={on} data-filter={`${title}:${value}`} onClick={() => onToggle(value)} className={`${CHIP} ${on ? "border-accent bg-surface-selected text-text" : "border-border text-text-muted hover:text-text"}`}>
              {labelOf?.(value) ?? value}
              {count !== undefined && <span className="font-mono">{count}</span>}
            </button>
          );
        })}
      </div>
    </fieldset>
  );
};

/** A field that takes effect when you leave it or press Enter, so a half-typed date doesn't search. An unreadable value stays here and goes nowhere. */
const CommitInput = ({ label, value, reads, placeholder, onCommit }: { readonly label: string; readonly value: string; readonly reads: (raw: string) => boolean; readonly placeholder: string; readonly onCommit: (value: string) => void }) => {
  const [draft, setDraft] = useState(value);
  useEffect(() => setDraft(value), [value]);
  const trimmed = draft.trim();
  const good = trimmed === "" || reads(trimmed);
  const commit = () => {
    if (good && trimmed !== value) onCommit(trimmed);
  };
  return (
    <Input
      label={label}
      value={draft}
      placeholder={placeholder}
      autoComplete="off"
      onChange={(e) => setDraft(e.target.value)}
      onBlur={commit}
      onKeyDown={(e) => {
        if (e.key === "Enter") commit();
      }}
      {...(good ? {} : { error: "Use a date such as 2024-05-31 or 2024-05-31T14:30, in UTC." })}
    />
  );
};

const SELECT = "h-8 rounded-md border border-border-strong bg-surface-raised px-2 text-sm text-text";

export interface FiltersProps {
  readonly state: SearchState;
  readonly facets: SearchFacets | null;
  readonly sourceName: (id: string) => string;
  readonly onChange: (next: SearchState) => void;
}

export const Filters = ({ state, facets, sourceName, onChange }: FiltersProps) => {
  const set = (change: Partial<SearchState>) => onChange(changeSearch(state, change));
  const past = state.asOf !== "";
  return (
    <aside aria-label="Filters" className="flex flex-col gap-3">
      <Chips title="Kind" values={SEARCH_KINDS} selected={state.kinds} facets={facets?.kind} labelOf={(v) => LABELS[v] ?? v} onToggle={(v) => set({ kinds: toggled(state.kinds, v) })} />
      <Chips title="Trust tier" values={TRUST_TIERS} selected={state.tiers} facets={facets?.trustTier} onToggle={(v) => set({ tiers: toggled(state.tiers, v) })} />
      <Chips title="Epistemic status" values={SEARCH_EPISTEMIC} selected={state.epistemic} facets={facets?.epistemic} onToggle={(v) => set({ epistemic: toggled(state.epistemic, v) })} />
      <Chips title="Type" values={[]} selected={state.types} facets={facets?.type} onToggle={(v) => set({ types: toggled(state.types, v) })} />
      <Chips title="Source" values={[]} selected={state.sources} facets={facets?.sourceId} labelOf={sourceName} onToggle={(v) => set({ sources: toggled(state.sources, v) })} />
      <CommitInput label="Valid at" value={state.validAt} placeholder="2024-05-31" reads={(raw) => parseInstant(raw) !== null} onCommit={(validAt) => set({ validAt })} />
      <CommitInput label="As of (an event number, or a date)" value={state.asOf} placeholder="1200 or 2024-05-31" reads={(raw) => parseAsOf(raw) !== null} onCommit={(asOf) => set({ asOf })} />
      <div className="flex flex-col gap-1">
        <label htmlFor="search-mode" className="text-xs text-text-muted">Signals</label>
        <select id="search-mode" className={SELECT} value={state.mode} onChange={(e) => set({ mode: e.target.value as SearchMode })}>
          {SEARCH_MODES.map((mode) => <option key={mode} value={mode} disabled={past && mode === "vector"}>{mode}</option>)}
        </select>
      </div>
      <div className="flex flex-col gap-1">
        <label htmlFor="search-expand" className="text-xs text-text-muted">Follow links between objects</label>
        <select id="search-expand" className={SELECT} value={state.expand} onChange={(e) => set({ expand: e.target.value as SearchExpand })}>
          {SEARCH_EXPANDS.map((expand) => <option key={expand} value={expand}>{expand}</option>)}
        </select>
      </div>
      <label className="flex items-center gap-2 text-sm">
        <input type="checkbox" checked={state.rerank} disabled={past} onChange={(e) => set({ rerank: e.target.checked })} />
        Rerank the top 20
      </label>
      {past && <p className="text-xs text-text-muted">A past state is read from the log, so it has no reranking or vectors.</p>}
      <label className="flex items-center gap-2 text-sm">
        <input type="checkbox" checked={state.debug} onChange={(e) => set({ debug: e.target.checked })} />
        Show debug details
      </label>
    </aside>
  );
};
