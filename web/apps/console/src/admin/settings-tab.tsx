import { settingKindText, settingRows } from "@memex/core";
import type { Usage } from "@memex/types";
import { Badge, Button, Input } from "@memex/ui";
import { useQuery } from "@tanstack/react-query";
import { useState } from "react";
import { useEngine } from "../engine-context";
import { QUERY_KEYS, unwrap } from "../query";
import { QueryView } from "../shell/query-view";
import { Panel } from "./admin-common";

const KIND_BADGE = { secret_set: "secret, set", secret_not_set: "secret, not set", redacted_url: "credentials hidden" } as const;

const SettingsTable = ({ tree }: { readonly tree: Readonly<Record<string, unknown>> }) => (
  <table aria-label="Effective settings" className="w-full text-left text-sm">
    <tbody>
      {settingRows(tree).map((r) => (
        <tr key={r.path} data-setting={r.path} data-kind={r.kind} className="border-t border-border align-top">
          <th scope="row" className="py-1 pr-3 font-mono text-xs font-normal text-text-muted">{r.path}</th>
          <td className="break-all font-mono text-xs">
            {r.kind === "secret_set" || r.kind === "secret_not_set" ? <span className="sr-only">{r.value}</span> : r.value}
            {(r.kind === "secret_set" || r.kind === "secret_not_set" || r.kind === "redacted_url") && (
              <> <Badge tone={r.kind === "secret_not_set" ? "neutral" : "warning"}>{KIND_BADGE[r.kind]}</Badge> <span className="font-sans text-text-muted">{settingKindText(r.kind)}</span></>
            )}
          </td>
        </tr>
      ))}
    </tbody>
  </table>
);

const DAY = /^\d{4}-\d{2}-\d{2}$/;

const UsageTable = ({ usage }: { readonly usage: Usage }) => {
  const total = usage.rows.reduce((sum, r) => sum + r.inputTokens + r.outputTokens + r.thinkingTokens, 0);
  return (
    <div className="flex flex-col gap-2">
      <p className="text-sm">
        {total} tokens in all. Cost where a price is set: <strong>${usage.pricedCostUsd.toFixed(4)}</strong>.
        {usage.unpricedTokens > 0 && <> {usage.unpricedTokens} tokens are for models with no price in <span className="font-mono">MEMEX_LLM_PRICES</span>, so they have no cost here.</>}
      </p>
      {usage.rows.length === 0 ? (
        <p className="text-sm text-text-muted">No LLM calls in this range.</p>
      ) : (
        <table className="w-full text-left text-sm">
          <thead className="text-xs text-text-muted"><tr><th className="py-1 pr-3 font-normal">Day</th><th className="pr-3 font-normal">Vault</th><th className="pr-3 font-normal">Agent</th><th className="pr-3 font-normal">Model</th><th className="pr-3 font-normal">Calls</th><th className="pr-3 font-normal">Input</th><th className="pr-3 font-normal">Output</th><th className="pr-3 font-normal">Thinking</th><th className="font-normal">Cost</th></tr></thead>
          <tbody>
            {usage.rows.map((r) => (
              <tr key={`${r.day}-${r.vaultId}-${r.agent}-${r.model}`} className="border-t border-border tabular-nums">
                <td className="py-1 pr-3 font-mono text-xs">{r.day}</td>
                <td className="pr-3 font-mono text-xs">{r.vaultId}</td>
                <td className="pr-3">{r.agent}</td>
                <td className="pr-3 font-mono text-xs">{r.model}</td>
                <td className="pr-3">{r.derivations}</td>
                <td className="pr-3">{r.inputTokens}</td>
                <td className="pr-3">{r.outputTokens}</td>
                <td className="pr-3">{r.thinkingTokens}</td>
                <td>{r.costUsd === null ? <span className="text-text-muted">no price</span> : `$${r.costUsd.toFixed(4)}`}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </div>
  );
};

const UsagePanel = () => {
  const engine = useEngine();
  const [from, setFrom] = useState("");
  const [to, setTo] = useState("");
  const [range, setRange] = useState<{ readonly from: string; readonly to: string }>({ from: "", to: "" });
  const bad = (d: string) => d !== "" && !DAY.test(d);
  const query = useQuery({ queryKey: QUERY_KEYS.usage(range.from, range.to), queryFn: async () => unwrap(await engine.getUsage(range.from === "" ? null : range.from, range.to === "" ? null : range.to)) });
  return (
    <div className="flex flex-col gap-3">
      <form
        aria-label="Usage range"
        className="flex flex-wrap items-end gap-2"
        onSubmit={(e) => {
          e.preventDefault();
          if (!bad(from) && !bad(to)) setRange({ from, to });
        }}
      >
        <Input label="From (YYYY-MM-DD, UTC)" value={from} onChange={(e) => setFrom(e.target.value)} {...(bad(from) ? { error: "A day as YYYY-MM-DD." } : {})} />
        <Input label="To (YYYY-MM-DD, UTC)" value={to} onChange={(e) => setTo(e.target.value)} {...(bad(to) ? { error: "A day as YYYY-MM-DD." } : {})} />
        <Button type="submit">Show</Button>
      </form>
      <QueryView query={query} label="the usage report">{(usage) => <UsageTable usage={usage} />}</QueryView>
    </div>
  );
};

export const SettingsTab = () => {
  const engine = useEngine();
  const query = useQuery({ queryKey: QUERY_KEYS.systemSettings, queryFn: async () => unwrap(await engine.getSystemSettings()) });
  return (
    <div className="flex flex-col gap-4">
      <Panel title="Effective settings" description="What this server runs with, read-only: settings are checked when the server starts, so they change with its environment and a restart. Secrets never leave the server, and show only whether they're set.">
        <QueryView query={query} label="the settings">{(tree) => <SettingsTable tree={tree} />}</QueryView>
      </Panel>
      <Panel title="LLM usage" description="Tokens by day, vault, agent and model, summed from what each call recorded. Review and audit calls aren't counted yet.">
        <UsagePanel />
      </Panel>
    </div>
  );
};
