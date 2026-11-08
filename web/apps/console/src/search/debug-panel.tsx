import type { EvidencePackage } from "@memex/types";

const score = (value: number | null): string => (value === null ? "-" : value.toFixed(4));
const rank = (value: number | null): string => (value === null ? "-" : String(value));

/** Why these results came back: the planner's path, the weights it chose, and each item's signals. */
export const DebugPanel = ({ result }: { readonly result: EvidencePackage }) => {
  const { debug } = result;
  if (debug === null) return <p className="text-sm text-text-muted">This answer has no debug details.</p>;
  const cell = "px-2 py-1 text-left font-normal";
  return (
    <section aria-label="Debug details" data-debug-panel={true} className="flex flex-col gap-3 rounded-lg border border-border bg-surface-raised p-3 text-xs">
      <h3 className="text-sm font-medium">Debug</h3>
      <dl className="grid grid-cols-[max-content_1fr] gap-x-3 gap-y-1">
        <dt className="text-text-muted">Planner's path</dt><dd className="font-mono" data-debug-path={true}>{result.path}</dd>
        <dt className="text-text-muted">Weights</dt><dd className="font-mono">lexical {debug.lexicalWeight.toFixed(2)}, vector {debug.vectorWeight.toFixed(2)}</dd>
        <dt className="text-text-muted">Terms</dt><dd className="font-mono">{debug.lexicalTerms} kept, {debug.droppedTerms} dropped{debug.termCoverage === null ? "" : `, coverage ${debug.termCoverage.toFixed(2)}`}</dd>
        <dt className="text-text-muted">Candidates</dt><dd className="font-mono">{debug.candidatesBeforeFence} before the fence, {debug.candidatesAfterFence} after, {debug.expanded} from Edges</dd>
        <dt className="text-text-muted">Best vector score</dt><dd className="font-mono">{score(debug.bestVectorScore)}</dd>
        <dt className="text-text-muted">Timings</dt><dd className="font-mono">{debug.timings.map((t) => `${t.stage} ${t.ms.toFixed(1)} ms`).join(", ")}</dd>
      </dl>
      <div className="overflow-x-auto">
        <table className="w-full font-mono">
          <caption className="sr-only">Each candidate's scores by signal</caption>
          <thead className="text-text-muted">
            <tr><th className={cell} scope="col">Id</th><th className={cell} scope="col">Fused</th><th className={cell} scope="col">Lexical rank</th><th className={cell} scope="col">Lexical</th><th className={cell} scope="col">Vector rank</th><th className={cell} scope="col">Vector</th><th className={cell} scope="col">Rerank</th></tr>
          </thead>
          <tbody>
            {debug.signals.map((s) => (
              <tr key={s.id} data-signal={s.id} className="border-t border-border">
                <td className="max-w-64 truncate px-2 py-1" title={s.id}>{s.id}</td>
                <td className="px-2 py-1">{s.fused.toFixed(4)}</td>
                <td className="px-2 py-1">{rank(s.lexicalRank)}</td>
                <td className="px-2 py-1">{score(s.lexicalScore)}</td>
                <td className="px-2 py-1">{rank(s.vectorRank)}</td>
                <td className="px-2 py-1">{score(s.vectorScore)}</td>
                <td className="px-2 py-1">{score(s.rerankScore)}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </section>
  );
};
