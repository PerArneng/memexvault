import { agentName, auditPath, costText, formatCount, monthName, monthOf, monthRange, shiftMonth, tokensTotal, usageView } from "@memex/core";
import type { Month, UsageTotals } from "@memex/core";
import { epochMs } from "@memex/types";
import type { Identity, VaultId } from "@memex/types";
import { EmptyState, StackedColumnChart } from "@memex/ui";
import { useQuery } from "@tanstack/react-query";
import { useEngine } from "../engine-context";
import { QUERY_KEYS, unwrap } from "../query";
import { PermissionMessage, QueryView } from "../shell/query-view";
import { AppLink } from "../wiki/wiki-common";

/** A cost only where the server priced every row in it. A sum of some priced rows says so, and none says nothing. */
const Cost = ({ totals }: { readonly totals: UsageTotals }) => {
  const text = costText(totals.costUsd);
  if (text === null) return <span className="text-text-faint" data-testid="cost">No price set</span>;
  return (
    <span data-testid="cost">
      {text}
      {totals.unpricedTokens > 0 && <span className="text-xs text-text-muted">, leaving out {formatCount(totals.unpricedTokens)} unpriced tokens</span>}
    </span>
  );
};

const TotalsRow = ({ label, totals, testId }: { readonly label: React.ReactNode; readonly totals: UsageTotals; readonly testId?: string }) => (
  <>
    <td className="p-1">{label}</td>
    <td className="p-1 text-right tabular-nums" data-testid={testId === undefined ? undefined : `${testId}-count`}>{formatCount(totals.count)}</td>
    <td className="p-1 text-right tabular-nums" data-testid={testId === undefined ? undefined : `${testId}-input`}>{formatCount(totals.tokens.input)}</td>
    <td className="p-1 text-right tabular-nums" data-testid={testId === undefined ? undefined : `${testId}-output`}>{formatCount(totals.tokens.output)}</td>
    <td className="p-1 text-right tabular-nums" data-testid={testId === undefined ? undefined : `${testId}-thinking`}>{formatCount(totals.tokens.thinking)}</td>
    <td className="p-1 text-right tabular-nums" data-testid={testId === undefined ? undefined : `${testId}-tokens`}>{formatCount(tokensTotal(totals.tokens))}</td>
    <td className="p-1 text-right"><Cost totals={totals} /></td>
  </>
);

const HEAD = (
  <thead className="text-left text-xs text-text-muted">
    <tr><th className="p-1">What</th><th className="p-1 text-right">Calls or runs</th><th className="p-1 text-right">Input</th><th className="p-1 text-right">Output</th><th className="p-1 text-right">Thinking</th><th className="p-1 text-right">Tokens</th><th className="p-1 text-right">Cost</th></tr>
  </thead>
);

const single = (count: number, tokens: { readonly input: number; readonly output: number; readonly thinking: number }, costUsd: number | null): UsageTotals => ({ count, tokens, costUsd, unpricedTokens: 0 });

export const AgentsView = ({ identity, vault, month }: { readonly identity: Identity; readonly vault: VaultId; readonly month: Month | null }) => {
  const engine = useEngine();
  const shown = month ?? monthOf(epochMs(engine.startedAt() + engine.uptimeMs()));
  const { from, to } = monthRange(shown);
  const admin = identity.principal.serverAdmin;
  const query = useQuery({ queryKey: QUERY_KEYS.agentUsage(from, to), queryFn: async () => unwrap(await engine.getAgentUsage(from, to)), enabled: admin });
  const nav = (
    <nav aria-label="Month" className="flex items-center gap-3 text-sm">
      <AppLink to={auditPath(vault, { kind: "agents", month: shiftMonth(shown, -1) })}>Previous month</AppLink>
      <h2 className="font-medium" data-testid="usage-month">{monthName(shown)}</h2>
      <AppLink to={auditPath(vault, { kind: "agents", month: shiftMonth(shown, 1) })}>Next month</AppLink>
    </nav>
  );
  if (!admin) {
    return (
      <div className="flex flex-col gap-3">
        {nav}
        <PermissionMessage title="Agent usage needs a server admin.">The usage report covers every vault on the server, so only a server admin can read it. Ask one to look, or for a token that is one.</PermissionMessage>
      </div>
    );
  }
  return (
    <div className="flex flex-col gap-3">
      {nav}
      <p className="text-sm text-text-muted">Tokens each agent spent in this vault, by model and UTC day: extraction, chat and audits. A cost shows only for a model the server has a price for, and is never guessed.</p>
      <QueryView query={query} label="agent usage">
        {(report) => {
          const view = usageView(report, vault, shown);
          if (view.rows.length === 0) return <EmptyState title={`No agent spent any tokens in this vault in ${monthName(shown)}.`} />;
          return (
            <div className="flex flex-col gap-4">
              <table className="w-full text-sm" data-testid="usage-totals">
                {HEAD}
                <tbody>
                  {view.groups.map((g) => (
                    <tr key={`${g.agent}|${g.model}`} className="border-t border-border" data-group={`${g.agent}|${g.model}`}>
                      <TotalsRow label={<><span className="font-medium">{agentName(g.agent)}</span> <span className="text-xs text-text-muted">{g.model}</span></>} totals={g.totals} />
                    </tr>
                  ))}
                  <tr className="border-t-2 border-border-strong font-medium" data-testid="usage-total-row">
                    <TotalsRow label="Total" totals={view.totals} testId="total" />
                  </tr>
                </tbody>
              </table>
              <StackedColumnChart
                label={`Tokens by UTC day and agent in ${monthName(shown)}. The table below has the exact numbers.`}
                series={view.agents.map(agentName)}
                columns={view.days.map((d) => ({ label: d.day.slice(8), values: d.byAgent }))}
                labelEvery={5}
              />
              <details>
                <summary className="text-sm text-text-muted">By day, agent and model: {formatCount(view.rows.length)} rows</summary>
                <table className="w-full text-sm" data-testid="usage-rows">
                  <thead className="text-left text-xs text-text-muted"><tr><th className="p-1">Day</th><th className="p-1">Agent and model</th><th className="p-1 text-right">Calls or runs</th><th className="p-1 text-right">Input</th><th className="p-1 text-right">Output</th><th className="p-1 text-right">Thinking</th><th className="p-1 text-right">Tokens</th><th className="p-1 text-right">Cost</th></tr></thead>
                  <tbody>
                    {view.rows.map((r) => (
                      <tr key={`${r.day}|${r.agent}|${r.model}`} className="border-t border-border">
                        <td className="p-1 font-mono text-xs">{r.day}</td>
                        <TotalsRow label={<>{agentName(r.agent)} <span className="text-xs text-text-muted">{r.model}</span></>} totals={single(r.count, r.tokens, r.costUsd)} />
                      </tr>
                    ))}
                  </tbody>
                </table>
              </details>
            </div>
          );
        }}
      </QueryView>
    </div>
  );
};
