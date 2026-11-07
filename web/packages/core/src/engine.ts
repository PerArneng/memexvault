import type { EpochMs, Identity, ObjectSummary, Vault, VaultId } from "@memex/types";
import { failed, isUnauthenticated, ok } from "./api";
import type { Api, ApiFailure, ApiResult } from "./api";
import type { Clock } from "./clock";
import { connectivityReducer, INITIAL_CONNECTIVITY, PROBE_TIMEOUT_MS } from "./connectivity";
import type { ConnectivityEvent, ConnectivityState } from "./connectivity";
import { noticeFor } from "./notices";
import type { FailureSource, Notice } from "./notices";
import { homeVault } from "./navigation";
import { LAST_VAULT_KEY } from "./preferences";
import type { Preferences } from "./preferences";
import { INITIAL_SESSION, sessionReducer } from "./session";
import type { SessionEvent, SessionState } from "./session";
import type { Settings } from "./settings";

export interface Edges {
  readonly clock: Clock;
  readonly settings: Settings;
  readonly api: Api;
  readonly preferences: Preferences;
}

/** A vault list can't run forever if a server hands back cursors in a loop: 50 pages of 200 is 10,000 vaults. */
const MAX_VAULT_PAGES = 50;

/** The single entry point for views. Later specs add a method per use case. */
export interface Engine {
  startedAt(): EpochMs;
  uptimeMs(): number;
  settings(): Settings;

  /** Called on any change to `session`, `connectivity` or `notices`. Returns the way to stop. */
  subscribe(listener: () => void): () => void;
  session(): SessionState;
  connectivity(): ConnectivityState;
  notices(): readonly Notice[];

  restoreSession(): Promise<void>;
  signIn(token: string): Promise<ApiResult<Identity>>;
  signOut(): Promise<ApiResult<null>>;

  listVaults(): Promise<ApiResult<readonly Vault[]>>;
  getVault(id: VaultId): Promise<ApiResult<Vault>>;
  listObjects(id: VaultId): Promise<ApiResult<readonly ObjectSummary[]>>;
  rememberVault(id: VaultId): void;
  /** The vault `/` should open, from those this token can see. */
  homeVault(vaults: readonly Vault[]): VaultId | null;

  /** One health check. Overlapping calls share the check in flight. */
  probe(): Promise<void>;

  reportFailure(failure: ApiFailure, source: FailureSource): void;
  dismissNotice(id: string): void;
}

export const createEngine = (edges: Edges): Engine => {
  const { api, clock, preferences } = edges;
  const started = clock.now();
  const listeners = new Set<() => void>();
  let session = INITIAL_SESSION;
  let connectivity = INITIAL_CONNECTIVITY;
  let notices: readonly Notice[] = [];
  let noticeCount = 0;
  let probing: Promise<void> | null = null;

  const notify = () => {
    for (const listener of [...listeners]) listener();
  };
  const dispatchSession = (event: SessionEvent) => {
    const next = sessionReducer(session, event);
    if (next !== session) {
      session = next;
      notify();
    }
  };
  const dispatchConnectivity = (event: ConnectivityEvent) => {
    const next = connectivityReducer(connectivity, event);
    if (next !== connectivity) {
      connectivity = next;
      notify();
    }
  };

  /** Every call goes through here, so an expired session or a dead server is noticed by whichever call meets it first. */
  const watched = async <T>(call: Promise<ApiResult<T>>): Promise<ApiResult<T>> => {
    const result = await call;
    if (!result.ok) {
      if (isUnauthenticated(result.failure)) dispatchSession({ type: "unauthenticated" });
      if (result.failure.kind === "unreachable") dispatchConnectivity({ type: "failed", at: clock.now() });
    }
    return result;
  };

  const restoreSession = async () => {
    dispatchSession({ type: "checking" });
    const result = await api.whoami();
    if (result.ok) dispatchSession({ type: "restored", identity: result.value });
    else if (isUnauthenticated(result.failure)) dispatchSession({ type: "no_session" });
    else dispatchSession({ type: "check_failed" });
  };

  const runProbe = async () => {
    const result = await api.health(PROBE_TIMEOUT_MS);
    dispatchConnectivity(result.ok ? { type: "reached" } : { type: "failed", at: clock.now() });
    // The first check found no server, so it's still open whether there's a session.
    if (result.ok && session.status === "unknown") await restoreSession();
  };

  return {
    startedAt: () => started,
    uptimeMs: () => clock.now() - started,
    settings: () => edges.settings,

    subscribe: (listener) => {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
    session: () => session,
    connectivity: () => connectivity,
    notices: () => notices,

    restoreSession,
    signIn: async (token) => {
      // Not through `watched`: a 401 here is a wrong token, and the form says so.
      const result = await api.signIn(token);
      if (result.ok) dispatchSession({ type: "signed_in", identity: result.value });
      return result;
    },
    signOut: async () => {
      const result = await watched(api.signOut());
      if (result.ok) dispatchSession({ type: "signed_out" });
      return result;
    },

    listVaults: async () => {
      const all: Vault[] = [];
      let cursor: string | null = null;
      for (let page = 0; page < MAX_VAULT_PAGES; page += 1) {
        const result: ApiResult<{ readonly items: readonly Vault[]; readonly nextCursor: string | null }> = await watched(api.listVaults(cursor));
        if (!result.ok) return failed(result.failure);
        all.push(...result.value.items);
        cursor = result.value.nextCursor;
        if (cursor === null) break;
      }
      return ok(all);
    },
    getVault: (id) => watched(api.getVault(id)),
    listObjects: (id) => watched(api.listObjects(id)),
    rememberVault: (id) => {
      preferences.set(LAST_VAULT_KEY, id);
    },
    homeVault: (vaults) => homeVault(vaults, preferences.get(LAST_VAULT_KEY)),

    probe: () => {
      probing ??= runProbe().finally(() => {
        probing = null;
      });
      return probing;
    },

    reportFailure: (failure, source) => {
      const notice = noticeFor(failure, source);
      if (notice === null) return;
      noticeCount += 1;
      notices = [...notices, { ...notice, id: `notice-${String(noticeCount)}` }];
      notify();
    },
    dismissNotice: (id) => {
      notices = notices.filter((n) => n.id !== id);
      notify();
    },
  };
};
