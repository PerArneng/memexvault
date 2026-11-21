import { mcpClientConfig, mcpStdioConfig } from "@memex/core";
import { useQuery } from "@tanstack/react-query";
import { useEngine } from "../engine-context";
import { QUERY_KEYS, unwrap } from "../query";
import { QueryView } from "../shell/query-view";
import { CodeBlock, CopyButton, KeyValues, Panel } from "./admin-common";

export const McpTab = () => {
  const engine = useEngine();
  const query = useQuery({ queryKey: QUERY_KEYS.mcp, queryFn: async () => unwrap(await engine.getMcpInfo()) });
  // The console is served by the server it talks to, so its own origin is the server's address as this browser reaches it.
  const origin = window.location.origin;
  return (
    <QueryView query={query} label="the MCP details">
      {(info) => {
        const http = mcpClientConfig(origin, info);
        const stdio = mcpStdioConfig();
        return (
          <div className="flex flex-col gap-4">
            <Panel title="Connecting a client" description="Give an agent its own principal and token on the Tokens screen, with only the access it needs, and put that token where the config says.">
              <KeyValues
                label="MCP endpoint"
                rows={[
                  ["Endpoint", <span className="font-mono" data-testid="mcp-endpoint">{`${origin}${info.endpoint}`}</span>],
                  ["Transports", info.transports.join(", ")],
                  ["Protocol", `${info.protocolVersion} (also ${info.supportedProtocolVersions.filter((v) => v !== info.protocolVersion).join(", ") || "no other"})`],
                  ["Answers hosts", info.allowedHosts.join(", ")],
                ]}
              />
              <h3 className="text-sm font-medium">Over HTTP</h3>
              <CodeBlock text={http} label="HTTP client config" />
              <div><CopyButton text={http} label="Copy the HTTP client config" /></div>
              <h3 className="text-sm font-medium">Over stdio, on the server's machine</h3>
              <CodeBlock text={stdio} label="Stdio client config" />
              <div><CopyButton text={stdio} label="Copy the stdio client config" /></div>
              <p className="text-xs text-text-muted">The HTTP endpoint answers only the hosts listed above, which <span className="font-mono">MEMEX_MCP_ALLOWED_HOSTS</span> sets. <span className="font-mono">memexvault mcp</span> refuses to start beside a running server.</p>
            </Panel>
            <Panel title={`Tools (${String(info.tools.length)})`}>
              <dl className="flex flex-col gap-2 text-sm">
                {info.tools.map((t) => (
                  <div key={t.name} data-tool={t.name}>
                    <dt className="font-mono text-text">{t.name}</dt>
                    <dd className="text-text-muted">{t.description}</dd>
                  </div>
                ))}
              </dl>
            </Panel>
          </div>
        );
      }}
    </QueryView>
  );
};
