import { expiryFrom, nameProblem, scopeIn, tokenState } from "@memex/core";
import type { NewToken } from "@memex/core";
import { SCOPES } from "@memex/types";
import type { CreatedToken, Grant, Identity, PrincipalRow, Scope, TokenRow, Vault, VaultId } from "@memex/types";
import { Badge, Button, Input } from "@memex/ui";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Plus, X } from "lucide-react";
import { useState } from "react";
import { useEngine } from "../engine-context";
import { QUERY_KEYS, unwrap } from "../query";
import { QueryView } from "../shell/query-view";
import { Callout, ConfirmPanel, CopyButton, failureText, formatWhen, Panel } from "./admin-common";

const SELECT = "h-8 rounded-md border border-border-strong bg-surface-raised px-2 text-sm text-text";

/** Shown once. Closing it drops the value from the page, and nothing else on the page or in the cache ever held it. */
const CreatedTokenPanel = ({ created, onClose }: { readonly created: CreatedToken; readonly onClose: () => void }) => (
  <section aria-label="New token" data-testid="new-token" className="flex flex-col gap-2 rounded-lg border border-warning bg-warning-surface p-3">
    <h3 className="font-medium">Token {created.details.name} created</h3>
    <p className="text-sm text-text-muted">Copy it now. This is the only time it's shown: the server keeps only its hash, and it won't appear here or anywhere else again.</p>
    <input aria-label="The new token" readOnly value={created.token} className="h-8 rounded-md border border-border-strong bg-surface-raised px-2 font-mono text-sm text-text" onFocus={(e) => e.currentTarget.select()} />
    <div className="flex items-center gap-2">
      <CopyButton text={created.token} label="Copy the new token" />
      <Button variant="primary" onClick={onClose}>Done, close</Button>
    </div>
  </section>
);

const GrantRows = ({ grants, vaults, onChange }: { readonly grants: readonly Grant[]; readonly vaults: readonly Vault[]; readonly onChange: (grants: readonly Grant[]) => void }) => (
  <fieldset className="flex flex-col gap-2">
    <legend className="mb-1 text-xs text-text-muted">Access</legend>
    {grants.map((g, i) => (
      <div key={i} className="flex items-center gap-2">
        <select aria-label={`Vault of grant ${String(i + 1)}`} className={SELECT} value={g.vault} onChange={(e) => onChange(grants.map((x, j) => (j === i ? { ...x, vault: e.target.value as VaultId } : x)))}>
          {vaults.map((v) => <option key={v.id} value={v.id}>{v.name}</option>)}
        </select>
        <select aria-label={`Scope of grant ${String(i + 1)}`} className={SELECT} value={g.scope} onChange={(e) => onChange(grants.map((x, j) => (j === i ? { ...x, scope: e.target.value as Scope } : x)))}>
          {SCOPES.map((s) => <option key={s} value={s}>{s}</option>)}
        </select>
        <Button variant="ghost" aria-label={`Remove grant ${String(i + 1)}`} onClick={() => onChange(grants.filter((_, j) => j !== i))}><X size={14} aria-hidden={true} /></Button>
      </div>
    ))}
    {vaults[0] !== undefined && (
      <div><Button onClick={() => onChange([...grants, { vault: vaults[0]?.id ?? ("" as VaultId), scope: "read" }])}><Plus size={14} aria-hidden={true} /> Add access to a vault</Button></div>
    )}
  </fieldset>
);

const CreateTokenForm = ({ identity, vault, vaults, principals, onCreated }: { readonly identity: Identity; readonly vault: VaultId; readonly vaults: readonly Vault[]; readonly principals: readonly PrincipalRow[] | null; readonly onCreated: (created: CreatedToken) => void }) => {
  const engine = useEngine();
  const queryClient = useQueryClient();
  // Only vaults this token may hand out access to: all for a server admin, and otherwise those it administers.
  const grantable = vaults.filter((v) => scopeIn(identity, v.id) === "admin");
  const [name, setName] = useState("");
  const [owner, setOwner] = useState("");
  const [grants, setGrants] = useState<readonly Grant[]>(grantable.some((v) => v.id === vault) ? [{ vault, scope: "read" }] : []);
  const [days, setDays] = useState("90");
  const [busy, setBusy] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const create = async () => {
    const expiry = expiryFrom(days, engine.startedAt() + engine.uptimeMs());
    const invalid = nameProblem(name) ?? (expiry.ok ? null : expiry.problem) ?? (grants.length === 0 && !identity.principal.serverAdmin ? "Give it access to at least one vault." : null);
    if (invalid !== null || !expiry.ok) {
      setProblem(invalid);
      return;
    }
    const input: NewToken = { name, principalId: owner.trim() === "" ? null : owner.trim(), grants, expiresAtMs: expiry.expiresAtMs };
    setBusy(true);
    setProblem(null);
    const result = await engine.createToken(input);
    setBusy(false);
    if (!result.ok) {
      setProblem(failureText(result.failure));
      return;
    }
    setName("");
    await queryClient.invalidateQueries({ queryKey: ["admin", "tokens"] });
    onCreated(result.value);
  };
  return (
    <form
      aria-label="Create a token"
      className="flex flex-col gap-3"
      onSubmit={(e) => {
        e.preventDefault();
        void create();
      }}
    >
      <div className="flex flex-wrap items-end gap-3">
        <Input label="Token name" value={name} onChange={(e) => setName(e.target.value)} />
        {principals !== null ? (
          <div className="flex flex-col gap-1">
            <label htmlFor="token-owner" className="text-xs text-text-muted">Whose token</label>
            <select id="token-owner" className={SELECT} value={owner} onChange={(e) => setOwner(e.target.value)}>
              <option value="">Yours ({identity.principal.name})</option>
              {principals.filter((p) => !p.disabled && p.id !== identity.principal.id).map((p) => <option key={p.id} value={p.id}>{p.name} ({p.kind}{p.serverAdmin ? ", server admin" : ""})</option>)}
            </select>
          </div>
        ) : (
          <Input label="Whose token: a principal's id, or blank for yours" value={owner} onChange={(e) => setOwner(e.target.value)} />
        )}
        <Input label="Expires in days (blank for never)" inputMode="numeric" value={days} onChange={(e) => setDays(e.target.value)} />
      </div>
      <GrantRows grants={grants} vaults={grantable} onChange={setGrants} />
      {problem !== null && <Callout tone="danger" title={problem} />}
      <div><Button type="submit" variant="primary" disabled={busy}>Create token</Button></div>
    </form>
  );
};

const STATE_TONE = { active: "success", revoked: "neutral", expired: "warning" } as const;

const TokenList = ({ identity, tokens, vaults, principals }: { readonly identity: Identity; readonly tokens: readonly TokenRow[]; readonly vaults: readonly Vault[]; readonly principals: readonly PrincipalRow[] | null }) => {
  const engine = useEngine();
  const queryClient = useQueryClient();
  const [revoking, setRevoking] = useState<string | null>(null);
  const now = engine.startedAt() + engine.uptimeMs();
  const vaultName = (id: string) => vaults.find((v) => v.id === id)?.name ?? id;
  const ownerName = (id: string) => (id === identity.principal.id ? `${identity.principal.name} (you)` : (principals?.find((p) => p.id === id)?.name ?? id));
  return (
    <div className="flex flex-col gap-2">
      <table className="w-full text-left text-sm">
        <thead className="text-xs text-text-muted">
          <tr><th className="py-1 pr-3 font-normal">Name</th><th className="pr-3 font-normal">Owner</th><th className="pr-3 font-normal">Access</th><th className="pr-3 font-normal">Expires</th><th className="pr-3 font-normal">Last used</th><th className="pr-3 font-normal">State</th><th className="font-normal"><span className="sr-only">Actions</span></th></tr>
        </thead>
        <tbody>
          {tokens.map((t) => {
            const state = tokenState(t, now);
            return (
              <tr key={t.id} data-token={t.id} className="border-t border-border align-top">
                <td className="py-1.5 pr-3">{t.name}{t.id === identity.tokenId && <> <Badge tone="info">this session</Badge></>}</td>
                <td className="pr-3">{ownerName(t.principalId)}</td>
                <td className="pr-3">{t.grants.length === 0 ? <span className="text-text-muted">none</span> : t.grants.map((g) => `${vaultName(g.vault)}: ${g.scope}`).join(", ")}</td>
                <td className="pr-3 font-mono text-xs">{formatWhen(t.expiresAtMs)}</td>
                <td className="pr-3 font-mono text-xs">{formatWhen(t.lastUsedAtMs)}</td>
                <td className="pr-3"><Badge tone={STATE_TONE[state]}>{state}</Badge></td>
                <td>{state !== "revoked" && revoking !== t.id && <Button variant="ghost" onClick={() => setRevoking(t.id)} aria-label={`Revoke ${t.name}`}>Revoke…</Button>}</td>
              </tr>
            );
          })}
        </tbody>
      </table>
      {tokens.length === 0 && <p className="text-sm text-text-muted">No tokens.</p>}
      {revoking !== null && (
        <ConfirmPanel
          title={`Revoke the token ${tokens.find((t) => t.id === revoking)?.name ?? revoking}`}
          description={revoking === identity.tokenId ? "This is the token you signed in with, so this session ends at once too." : "Anything using it, and any console session signed in with it, is refused from the next request. It can't be undone."}
          confirmLabel="Revoke"
          run={async () => {
            const result = await engine.revokeToken(revoking);
            return result.ok ? null : result.failure;
          }}
          onDone={() => {
            setRevoking(null);
            void queryClient.invalidateQueries({ queryKey: ["admin", "tokens"] });
          }}
          onCancel={() => setRevoking(null)}
        />
      )}
    </div>
  );
};

const PrincipalsPanel = ({ identity, principals }: { readonly identity: Identity; readonly principals: readonly PrincipalRow[] }) => {
  const engine = useEngine();
  const queryClient = useQueryClient();
  const [name, setName] = useState("");
  const [kind, setKind] = useState<"human" | "agent">("human");
  const [serverAdmin, setServerAdmin] = useState(false);
  const [problem, setProblem] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [disabling, setDisabling] = useState<PrincipalRow | null>(null);
  const create = async () => {
    const invalid = nameProblem(name);
    if (invalid !== null) {
      setProblem(invalid);
      return;
    }
    setBusy(true);
    const result = await engine.createPrincipal({ name, kind, serverAdmin });
    setBusy(false);
    setProblem(result.ok ? null : failureText(result.failure));
    if (result.ok) {
      setName("");
      setServerAdmin(false);
      await queryClient.invalidateQueries({ queryKey: QUERY_KEYS.principals });
    }
  };
  return (
    <div className="flex flex-col gap-3">
      <table className="w-full text-left text-sm">
        <thead className="text-xs text-text-muted"><tr><th className="py-1 pr-3 font-normal">Name</th><th className="pr-3 font-normal">Kind</th><th className="pr-3 font-normal">Created</th><th className="pr-3 font-normal">State</th><th className="font-normal"><span className="sr-only">Actions</span></th></tr></thead>
        <tbody>
          {principals.map((p) => (
            <tr key={p.id} data-principal={p.id} className="border-t border-border">
              <td className="py-1.5 pr-3">{p.name}{p.serverAdmin && <> <Badge tone="warning">server admin</Badge></>}</td>
              <td className="pr-3">{p.kind}</td>
              <td className="pr-3 font-mono text-xs">{formatWhen(p.createdAtMs)}</td>
              <td className="pr-3">{p.disabled ? <Badge tone="neutral">disabled</Badge> : <Badge tone="success">active</Badge>}</td>
              <td>{!p.disabled && p.id !== identity.principal.id && <Button variant="ghost" aria-label={`Disable ${p.name}`} onClick={() => setDisabling(p)}>Disable…</Button>}</td>
            </tr>
          ))}
        </tbody>
      </table>
      {disabling !== null && (
        <ConfirmPanel
          title={`Disable ${disabling.name}`}
          description="Every token it holds stops working at once, and it can't be turned back on from the console."
          typed={{ expected: disabling.name, label: `Type the principal's name, ${disabling.name}, to confirm` }}
          confirmLabel="Disable"
          run={async () => {
            const result = await engine.disablePrincipal(disabling.id);
            return result.ok ? null : result.failure;
          }}
          onDone={() => {
            setDisabling(null);
            void queryClient.invalidateQueries({ queryKey: QUERY_KEYS.principals });
          }}
          onCancel={() => setDisabling(null)}
        />
      )}
      <form
        aria-label="Add a principal"
        className="flex flex-wrap items-end gap-3"
        onSubmit={(e) => {
          e.preventDefault();
          void create();
        }}
      >
        <Input label="Principal's name" value={name} onChange={(e) => setName(e.target.value)} />
        <fieldset className="flex items-center gap-3 text-sm">
          <legend className="sr-only">Kind</legend>
          <label className="inline-flex items-center gap-1"><input type="radio" name="principal-kind" checked={kind === "human"} onChange={() => setKind("human")} /> Person</label>
          <label className="inline-flex items-center gap-1"><input type="radio" name="principal-kind" checked={kind === "agent"} onChange={() => setKind("agent")} /> Agent</label>
        </fieldset>
        <label className="inline-flex items-center gap-1 text-sm"><input type="checkbox" checked={serverAdmin} onChange={(e) => setServerAdmin(e.target.checked)} /> Server admin</label>
        <Button type="submit" variant="primary" disabled={busy}>Add principal</Button>
      </form>
      {problem !== null && <Callout tone="danger" title={problem} />}
    </div>
  );
};

export const TokensTab = ({ identity, vault }: { readonly identity: Identity; readonly vault: VaultId }) => {
  const engine = useEngine();
  const serverAdmin = identity.principal.serverAdmin;
  const [created, setCreated] = useState<CreatedToken | null>(null);
  const vaultsQuery = useQuery({ queryKey: QUERY_KEYS.vaults, queryFn: async () => unwrap(await engine.listVaults()) });
  const tokensQuery = useQuery({ queryKey: QUERY_KEYS.tokens(serverAdmin), queryFn: async () => unwrap(await engine.listTokens(serverAdmin)) });
  const principalsQuery = useQuery({ queryKey: QUERY_KEYS.principals, queryFn: async () => unwrap(await engine.listPrincipals()), enabled: serverAdmin });
  const principals = serverAdmin ? (principalsQuery.data ?? null) : null;
  return (
    <div className="flex flex-col gap-4">
      <Panel title="Create a token" description="A token gives its owner the access listed here. Server admins have every scope in every vault whatever a token lists.">
        {created !== null ? (
          <CreatedTokenPanel created={created} onClose={() => setCreated(null)} />
        ) : (
          <QueryView query={vaultsQuery} label="your vaults">
            {(vaults) => <CreateTokenForm identity={identity} vault={vault} vaults={vaults} principals={principals} onCreated={setCreated} />}
          </QueryView>
        )}
      </Panel>
      <Panel title={serverAdmin ? "All tokens" : "Your tokens"}>
        <QueryView query={tokensQuery} label="the tokens">
          {(tokens) => <TokenList identity={identity} tokens={tokens} vaults={vaultsQuery.data ?? []} principals={principals} />}
        </QueryView>
      </Panel>
      {serverAdmin && (
        <Panel title="Principals" description="People and agents. Each token belongs to one, and everything it does is recorded as theirs.">
          <QueryView query={principalsQuery} label="the principals">
            {(rows) => <PrincipalsPanel identity={identity} principals={rows} />}
          </QueryView>
        </Panel>
      )}
    </div>
  );
};
