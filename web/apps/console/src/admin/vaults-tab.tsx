import { adminPath, defaultSectionPath } from "@memex/core";
import type { Identity } from "@memex/types";
import { useQuery } from "@tanstack/react-query";
import { useEngine } from "../engine-context";
import { QUERY_KEYS, unwrap } from "../query";
import { QueryView } from "../shell/query-view";
import { AppLink, useGo } from "../wiki/wiki-common";
import { formatWhen, Panel } from "./admin-common";
import { CreateVaultForm } from "./create-vault";

export const VaultsTab = ({ identity }: { readonly identity: Identity }) => {
  const engine = useEngine();
  const go = useGo();
  const query = useQuery({ queryKey: QUERY_KEYS.vaults, queryFn: async () => unwrap(await engine.listVaults()) });
  return (
    <div className="flex flex-col gap-4">
      <Panel title="Create a vault" description="Each vault is isolated: its own schema in Postgres, its own files and its own access.">
        <CreateVaultForm onCreated={(v) => go(adminPath(v.id, "vault"))} />
      </Panel>
      <Panel title="Every vault">
        <QueryView query={query} label="vaults" isEmpty={(rows) => rows.length === 0}>
          {(rows) => (
            <table className="w-full text-left text-sm">
              <thead className="text-xs text-text-muted"><tr><th className="py-1 pr-3 font-normal">Name</th><th className="pr-3 font-normal">Id</th><th className="pr-3 font-normal">Created</th><th className="font-normal"><span className="sr-only">Actions</span></th></tr></thead>
              <tbody>
                {rows.map((v) => (
                  <tr key={v.id} data-vault={v.id} className="border-t border-border">
                    <td className="py-1.5 pr-3"><AppLink to={defaultSectionPath(identity, v.id)}>{v.name}</AppLink></td>
                    <td className="pr-3 font-mono text-xs">{v.id}</td>
                    <td className="pr-3 font-mono text-xs">{formatWhen(v.createdAt)}</td>
                    <td><AppLink to={adminPath(v.id, "vault")}>Manage</AppLink></td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
        </QueryView>
      </Panel>
    </div>
  );
};
