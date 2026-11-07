import { defaultSectionPath, parseVaultPath, scopeIn, sectionPath, switchVaultPath, vaultPath, visibleSections } from "@memex/core";
import type { Identity, Vault, VaultId } from "@memex/types";
import { Button, EmptyState, Menu, Skeleton, Toasts } from "@memex/ui";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useLocation, useNavigate, useRouter } from "@tanstack/react-router";
import { ChevronDown, Search as SearchIcon } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";
import { useConnectivity, useEngine, useNotices, useSession } from "../engine-context";
import { Frame } from "../frame";
import { QUERY_KEYS, unwrap } from "../query";
import { Palette } from "./palette";
import { QueryView } from "./query-view";
import { SectionScreen } from "./section-screen";

const VaultSwitcher = ({ identity, vault, vaults }: { readonly identity: Identity; readonly vault: VaultId | null; readonly vaults: readonly Vault[] }) => {
  const navigate = useNavigate();
  const { pathname } = useLocation();
  const current = vaults.find((v) => v.id === vault);
  return (
    <Menu
      trigger={
        <button type="button" aria-label={`Vault: ${current?.name ?? "none chosen"}. Switch vault`} className="flex h-8 w-full items-center justify-between gap-2 rounded-md border border-border-strong bg-surface-raised px-2 text-sm">
          <span className="truncate">{current?.name ?? "Choose a vault"}</span>
          <ChevronDown size={14} aria-hidden={true} className="shrink-0" />
        </button>
      }
      items={vaults.map((v) => ({ id: v.id, label: v.name, onSelect: () => void navigate({ to: switchVaultPath(identity, pathname, v.id) }) }))}
    />
  );
};

const SideNav = ({ identity, vault, section }: { readonly identity: Identity; readonly vault: VaultId; readonly section: string | null }) => (
  <nav aria-label="Screens" className="flex flex-col gap-0.5">
    {visibleSections(identity, vault).map((s) => (
      <Link
        key={s.id}
        to={sectionPath(vault, s.id)}
        aria-current={s.id === section ? "page" : undefined}
        className={`rounded-md px-2 py-1 text-sm ${s.id === section ? "bg-surface-selected text-text" : "text-text-muted hover:bg-surface-overlay hover:text-text"}`}
      >
        {s.label}
      </Link>
    ))}
  </nav>
);

/** Refetches what failed when the connection returns, and drops one user's data when their session ends. */
const useCacheLifecycle = () => {
  const queryClient = useQueryClient();
  const connectivity = useConnectivity();
  const wasOffline = useRef(false);
  useEffect(() => {
    if (connectivity.status === "offline") wasOffline.current = true;
    else if (wasOffline.current) {
      wasOffline.current = false;
      void queryClient.invalidateQueries();
    }
  }, [connectivity.status, queryClient]);
  useEffect(() => () => queryClient.clear(), [queryClient]);
};

const Home = ({ identity, vaults }: { readonly identity: Identity; readonly vaults: readonly Vault[] }) => {
  const engine = useEngine();
  const router = useRouter();
  const target = engine.homeVault(vaults);
  useEffect(() => {
    if (target !== null) router.history.replace(defaultSectionPath(identity, target));
  }, [identity, router, target]);
  if (target !== null) return <Skeleton className="h-24 w-full" />;
  return (
    <section className="flex flex-col gap-4" aria-labelledby="screen-title">
      <h1 id="screen-title" className="text-xl font-medium">No vaults</h1>
      <EmptyState title="There are no vaults you can open yet.">
        {identity.principal.serverAdmin ? "Create one through the API, as the console's administration screens aren't built yet." : "Ask an administrator to give your token access to one."}
      </EmptyState>
    </section>
  );
};

const Shell = () => {
  const engine = useEngine();
  const session = useSession();
  const notices = useNotices();
  const location = useLocation();
  const router = useRouter();
  const [paletteOpen, setPaletteOpen] = useState(false);
  useCacheLifecycle();

  const vaultsQuery = useQuery({ queryKey: QUERY_KEYS.vaults, queryFn: async () => unwrap(await engine.listVaults()) });
  const signOut = useMutation({ mutationFn: async () => unwrap(await engine.signOut()) });
  const startSignOut = useCallback(() => signOut.mutate(), [signOut]);

  const place = parseVaultPath(location.pathname);
  const vault = place?.vault ?? null;
  useEffect(() => {
    if (vault !== null) engine.rememberVault(vault);
  }, [engine, vault]);
  useEffect(() => {
    if (place !== null && place.section === null && session.status === "signed_in") {
      router.history.replace(defaultSectionPath(session.identity, place.vault));
    }
  }, [place, router, session]);

  // The guard unmounts this as soon as the session ends, so there's always an identity to render with until then.
  if (session.status !== "signed_in") return null;
  const { identity } = session;
  const vaults = vaultsQuery.data ?? [];
  const scope = vault === null ? null : scopeIn(identity, vault);

  return (
    <Frame>
      <aside className="flex w-56 shrink-0 flex-col gap-4 border-r border-border bg-surface-raised p-3">
        <Link to={vault === null ? "/" : vaultPath(vault)} className="px-2 text-lg font-medium">memexvault</Link>
        <div className="flex flex-col gap-1">
          <span className="px-2 text-xs text-text-muted">Vault</span>
          <VaultSwitcher identity={identity} vault={vault} vaults={vaults} />
        </div>
        <Button onClick={() => setPaletteOpen(true)} className="justify-between">
          <span className="inline-flex items-center gap-1.5"><SearchIcon size={14} aria-hidden={true} /> Commands</span>
          <kbd className="font-mono text-xs text-text-muted">Ctrl K</kbd>
        </Button>
        <div className="min-h-0 flex-1 overflow-auto">
          {vault !== null && <SideNav identity={identity} vault={vault} section={place?.section ?? null} />}
        </div>
        <div className="flex flex-col gap-2 border-t border-border pt-3">
          <p className="truncate px-2 text-xs text-text-muted" title={identity.principal.name}>
            <span className="text-text">{identity.principal.name}</span>
            {scope !== null && <> · {identity.principal.serverAdmin ? "server admin" : scope}</>}
          </p>
          <Button variant="ghost" onClick={startSignOut} disabled={signOut.isPending}>Sign out</Button>
        </div>
      </aside>
      <main className="min-w-0 flex-1 overflow-auto p-6">
        {place === null ? (
          <QueryView query={vaultsQuery} label="your vaults">
            {(data) => <Home identity={identity} vaults={data} />}
          </QueryView>
        ) : place.section === null ? (
          <Skeleton className="h-24 w-full" />
        ) : (
          <SectionScreen identity={identity} vault={place.vault} sectionName={place.section} objectId={place.rest[0]} />
        )}
      </main>
      <Palette open={paletteOpen} onOpenChange={setPaletteOpen} identity={identity} vault={vault} vaults={vaults} onSignOut={startSignOut} />
      <Toasts messages={notices} onDismiss={engine.dismissNotice} />
    </Frame>
  );
};

export default Shell;
