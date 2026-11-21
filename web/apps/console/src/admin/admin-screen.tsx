import { ADMIN_TABS, adminAccess, adminAccessText, adminPath, adminTabById, parseAdminRoute } from "@memex/core";
import type { Identity, VaultId } from "@memex/types";
import { EmptyState } from "@memex/ui";
import { PermissionMessage } from "../shell/query-view";
import { AppLink } from "../wiki/wiki-common";
import { ErasureTab } from "./erasure-tab";
import { McpTab } from "./mcp-tab";
import { OperationsTab } from "./operations-tab";
import { SettingsTab } from "./settings-tab";
import { StatusTab } from "./status-tab";
import { TokensTab } from "./tokens-tab";
import { VaultsTab } from "./vaults-tab";
import { VaultTab } from "./vault-tab";

/**
 * The admin section. It reads nothing before deciding what the URL may show, and the status screen needs nothing but its own route, so it
 * still explains a Postgres outage when every other read fails.
 */
const AdminScreen = ({ identity, vault, rest }: { readonly identity: Identity; readonly vault: VaultId; readonly rest: readonly string[] }) => {
  const route = parseAdminRoute(rest);
  const current = route.kind === "tab" ? route.tab : null;
  const tab = current === null ? null : adminTabById(current);
  const access = tab === null ? null : adminAccess(identity, vault, tab);
  return (
    <div className="flex flex-col gap-3">
      <nav aria-label="Administration" className="flex flex-wrap gap-4 border-b border-border pb-1 text-sm">
        {ADMIN_TABS.filter((t) => adminAccess(identity, vault, t).kind === "allowed" || t.id === current).map((t) => (
          <AppLink key={t.id} to={adminPath(vault, t.id)} aria-current={t.id === current ? "page" : undefined} className={t.id === current ? "font-medium text-text underline" : "text-accent underline"}>{t.label}</AppLink>
        ))}
      </nav>
      {route.kind === "unknown" && <EmptyState title="There's nothing at this address."><AppLink to={adminPath(vault, "vault")}>Go to the vault's settings</AppLink></EmptyState>}
      {tab !== null && access !== null && access.kind !== "allowed" && (
        <PermissionMessage title={`You don't have permission to use ${tab.label} here.`}>{adminAccessText(access)}</PermissionMessage>
      )}
      {route.kind === "tab" && access?.kind === "allowed" && (
        <>
          {route.tab === "vault" && <VaultTab vault={vault} />}
          {route.tab === "tokens" && <TokensTab identity={identity} vault={vault} />}
          {route.tab === "operations" && <OperationsTab identity={identity} vault={vault} id={route.id} />}
          {route.tab === "erasure" && <ErasureTab key={route.id ?? ""} vault={vault} id={route.id} />}
          {route.tab === "status" && <StatusTab />}
          {route.tab === "settings" && <SettingsTab />}
          {route.tab === "mcp" && <McpTab />}
          {route.tab === "vaults" && <VaultsTab identity={identity} />}
        </>
      )}
    </div>
  );
};

export default AdminScreen;
