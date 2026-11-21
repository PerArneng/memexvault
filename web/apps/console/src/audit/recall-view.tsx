import { formatCount, recallText, recallWarnings, targetText } from "@memex/core";
import type { GroupRecall, VaultId } from "@memex/types";
import { Badge } from "@memex/ui";
import { useQuery } from "@tanstack/react-query";
import { useEngine } from "../engine-context";
import { QUERY_KEYS, unwrap } from "../query";
import { QueryView } from "../shell/query-view";
import { Callout } from "../wiki/wiki-common";

const Group = ({ title, group, testId }: { readonly title: string; readonly group: GroupRecall; readonly testId: string }) => (
  <div className="flex flex-col rounded-md border border-border p-2" data-testid={testId}>
    <span className="text-xs text-text-muted">{title}</span>
    <span className="text-lg tabular-nums">{recallText(group.recall)}</span>
    <span className="text-xs text-text-muted tabular-nums">{formatCount(group.caught)} caught of {formatCount(group.audited)} audited, {formatCount(group.planted)} planted</span>
  </div>
);

export const RecallView = ({ vault }: { readonly vault: VaultId }) => {
  const engine = useEngine();
  const query = useQuery({ queryKey: QUERY_KEYS.auditRecall(vault), queryFn: async () => unwrap(await engine.getAuditRecall(vault)) });
  return (
    <QueryView query={query} label="the recall report">
      {(r) => (
        <div className="flex flex-col gap-3">
          <p className="text-sm text-text-muted">Recall is measured in a test vault, by planting known errors and seeing how many audits catch. Kinds the deterministic checks catch are kept apart from kinds only an LLM check can.</p>
          {recallWarnings(r).map((w) => (
            <div key={w.title} data-testid="recall-warning"><Callout tone="warning" title={w.title}>{w.detail}</Callout></div>
          ))}
          <div className="flex flex-wrap items-baseline gap-3">
            <span className="text-2xl tabular-nums" data-testid="overall-recall">{recallText(r.overallRecall)}</span>
            <span className="text-sm text-text-muted" data-testid="sample-size">overall, from a sample of {formatCount(r.overallPlanted)} planted errors ({formatCount(r.overallCaught)} caught) and {formatCount(r.cleanAudited)} clean claims audited</span>
          </div>
          <p className="text-sm" data-testid="target">{targetText(r)}</p>
          <div className="grid grid-cols-1 gap-2 md:grid-cols-2">
            <Group title="Deterministic checks" group={r.deterministic} testId="group-deterministic" />
            <Group title={`LLM checks only${r.llmModels.length === 0 ? "" : `, by ${r.llmModels.join(", ")}`}`} group={r.llmOnly} testId="group-llm" />
          </div>
          <p className="text-sm text-text-muted">False positives: {formatCount(r.falsePositivesDeterministic)} from deterministic checks and {formatCount(r.falsePositivesLlm)} from LLM checks.</p>
          {r.kinds.length > 0 && (
            <table className="w-full text-sm" data-testid="recall-kinds">
              <thead className="text-left text-xs text-text-muted"><tr><th className="p-1">Kind</th><th className="p-1">Caught by</th><th className="p-1 text-right">Planted</th><th className="p-1 text-right">Audited</th><th className="p-1 text-right">Caught</th><th className="p-1 text-right">Recall</th></tr></thead>
              <tbody>
                {r.kinds.map((k) => (
                  <tr key={k.kind} className="border-t border-border">
                    <td className="p-1 font-mono text-xs">{k.kind}</td>
                    <td className="p-1">{k.needsLlm ? <Badge tone="info">LLM only</Badge> : <Badge>Deterministic</Badge>}</td>
                    <td className="p-1 text-right tabular-nums">{formatCount(k.planted)}</td>
                    <td className="p-1 text-right tabular-nums">{formatCount(k.audited)}</td>
                    <td className="p-1 text-right tabular-nums">{formatCount(k.caught)}</td>
                    <td className="p-1 text-right tabular-nums">{recallText(k.recall)}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </div>
      )}
    </QueryView>
  );
};
