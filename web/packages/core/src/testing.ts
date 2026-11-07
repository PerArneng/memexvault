import { epochMs, vaultId } from "@memex/types";
import type { EpochMs, Grant, Identity, ObjectSummary, Page, Vault, VaultId } from "@memex/types";
import { failed, ok } from "./api";
import type { Api, ApiFailure, ApiResult } from "./api";
import type { Clock } from "./clock";
import type { Preferences } from "./preferences";

/** A clock tests move by hand, so nothing sleeps. */
export interface ManualClock extends Clock {
  advance(ms: number): void;
}

export const manualClock = (start = 0): ManualClock => {
  let current = start;
  return {
    now: (): EpochMs => epochMs(current),
    advance: (ms) => {
      current += ms;
    },
  };
};

export const memoryPreferences = (initial: Readonly<Record<string, string>> = {}): Preferences => {
  const values = new Map(Object.entries(initial));
  return {
    get: (key) => values.get(key) ?? null,
    set: (key, value) => {
      values.set(key, value);
    },
  };
};

export const identityWith = (grants: readonly Grant[], serverAdmin = false): Identity => ({
  principal: { id: "p1", name: "pat", serverAdmin },
  tokenId: "t1",
  grants,
});

export const vaultNamed = (id: string, name = id): Vault => ({ id: vaultId(id), name, createdAt: epochMs(0) });

export const httpFailure = (status: number, code: string, message = code): ApiFailure => ({ kind: "http", status, code, message, requestId: "req-1" });

export const UNREACHABLE: ApiFailure = { kind: "unreachable", message: "no answer" };

export interface FakeApiSeed {
  /** Which token signs in as whom. */
  readonly tokens?: Readonly<Record<string, Identity>>;
  readonly vaults?: readonly Vault[];
  readonly objects?: Readonly<Record<string, readonly ObjectSummary[]>>;
  /** Vaults per page, to exercise cursors. */
  readonly pageSize?: number;
  /** Starts signed in as this identity. */
  readonly session?: Identity;
}

/** An in-memory server with seeded data. `down` and `failNext` script the bad cases. */
export interface FakeApi extends Api {
  down: boolean;
  failNext(failure: ApiFailure): void;
  /** Ends the session on the server's side, as expiry or revocation would. */
  endSession(): void;
  readonly calls: string[];
}

export const fakeApi = (seed: FakeApiSeed = {}): FakeApi => {
  const vaults = seed.vaults ?? [];
  const pageSize = seed.pageSize ?? 200;
  let session: Identity | null = seed.session ?? null;
  const scripted: ApiFailure[] = [];
  const calls: string[] = [];

  const answer = async <T>(name: string, value: () => ApiResult<T>): Promise<ApiResult<T>> => {
    calls.push(name);
    const next = scripted.shift();
    if (next !== undefined) return failed<T>(next);
    if (api.down) return failed<T>(UNREACHABLE);
    return value();
  };
  const unauthenticated = httpFailure(401, "unauthenticated");
  const notFound = httpFailure(404, "not_found");

  const api: FakeApi = {
    down: false,
    calls,
    failNext: (failure) => {
      scripted.push(failure);
    },
    endSession: () => {
      session = null;
    },
    whoami: () => answer("whoami", () => (session === null ? failed<Identity>(unauthenticated) : ok(session))),
    signIn: (token) =>
      answer("signIn", () => {
        const identity = seed.tokens?.[token];
        if (identity === undefined) return failed<Identity>(unauthenticated);
        session = identity;
        return ok(identity);
      }),
    signOut: () =>
      answer("signOut", () => {
        session = null;
        return ok(null);
      }),
    listVaults: (cursor) =>
      answer("listVaults", () => {
        if (session === null) return failed<Page<Vault>>(unauthenticated);
        const start = cursor === null ? 0 : Number(cursor);
        const end = start + pageSize;
        return ok({ items: vaults.slice(start, end), nextCursor: end < vaults.length ? String(end) : null });
      }),
    getVault: (id: VaultId) =>
      answer("getVault", () => {
        if (session === null) return failed<Vault>(unauthenticated);
        const vault = vaults.find((v) => v.id === id);
        return vault === undefined ? failed<Vault>(notFound) : ok(vault);
      }),
    listObjects: (id: VaultId) =>
      answer("listObjects", () => (session === null ? failed<readonly ObjectSummary[]>(unauthenticated) : ok(seed.objects?.[id] ?? []))),
    health: () => answer("health", () => ok(null)),
  };
  return api;
};
