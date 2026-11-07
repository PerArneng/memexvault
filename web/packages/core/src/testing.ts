import { epochMs, vaultId } from "@memex/types";
import type { EpochMs, Grant, Identity, ObjectSummary, Page, Vault, VaultId, WikiClaim, WikiPage, WikiPageSummary, WikiRevision, WikiWrite } from "@memex/types";
import { failed, ok } from "./api";
import type { Api, ApiFailure, ApiResult } from "./api";
import type { Clock } from "./clock";
import type { DraftStorage } from "./draft";
import type { Preferences } from "./preferences";
import { slugify } from "./wikitext";
import type { IdSource } from "./wiki-cases";

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

/** `failing` scripts a full or blocked browser. */
export const memoryDraftStorage = (initial: Readonly<Record<string, string>> = {}): DraftStorage & { failing: boolean; readonly values: Map<string, string> } => {
  const values = new Map(Object.entries(initial));
  const storage = {
    failing: false,
    values,
    get: (key: string) => values.get(key) ?? null,
    set: (key: string, value: string) => {
      if (storage.failing) return false;
      values.set(key, value);
      return true;
    },
    remove: (key: string) => {
      values.delete(key);
    },
  };
  return storage;
};

export const countingIds = (): IdSource => {
  let count = 0;
  return {
    next: () => {
      count += 1;
      return `id-${String(count)}`;
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
  /** Claims `readClaim` knows, each with what replaced it. */
  readonly claims?: Readonly<Record<string, { readonly claim: WikiClaim; readonly supersededBy: string | null }>>;
  /** What `searchClaimIds` finds, whatever is asked. */
  readonly claimHits?: readonly string[];
  /** Pages per list page, to exercise cursors. */
  readonly wikiPageSize?: number;
  /** Writes wait for review, as an agent's do. */
  readonly wikiWritesPending?: boolean;
}

/** An in-memory server with seeded data. `down` and `failNext` script the bad cases. */
export interface FakeApi extends Api {
  down: boolean;
  failNext(failure: ApiFailure): void;
  /** Ends the session on the server's side, as expiry or revocation would. */
  endSession(): void;
  readonly calls: string[];
  /** Every idempotency key a write has used. */
  readonly writeKeys: string[];
  /** Writes as another session would, without a stale check. */
  otherSessionEdits(id: string, body: string): void;
  /** The body as stored. */
  wikiBody(id: string): string | undefined;
  /** The next write lands, but its answer never comes back. */
  loseNextAnswer(): void;
}

export const fakeApi = (seed: FakeApiSeed = {}): FakeApi => {
  const vaults = seed.vaults ?? [];
  const pageSize = seed.pageSize ?? 200;
  let session: Identity | null = seed.session ?? null;
  const scripted: ApiFailure[] = [];
  const calls: string[] = [];
  const writeKeys: string[] = [];
  /** Each page's revisions, oldest first. Index n holds revision n + 1. */
  const wiki = new Map<string, { title: string; slug: string; bodies: string[] }>();
  const replays = new Map<string, WikiWrite>();
  let losing = false;
  const wikiPageSize = seed.wikiPageSize ?? 200;

  const wikiSummary = (id: string, p: { title: string; slug: string; bodies: string[] }): WikiPageSummary => ({ id, title: p.title, slug: p.slug, redirects: [], version: p.bodies.length, creator: "pat", updatedAt: epochMs(p.bodies.length) });
  const wikiView = (id: string, p: { title: string; slug: string; bodies: string[] }, revision: number): WikiPage => ({
    id,
    currentRevision: p.bodies.length,
    currentSlug: p.slug,
    redirects: [],
    revision,
    title: p.title,
    slug: p.slug,
    body: p.bodies[revision - 1] ?? "",
    claims: [],
    creator: "pat",
    updatedAt: epochMs(revision),
    restoredFrom: null,
  });
  const staleFailure = httpFailure(409, "stale_version");
  const committed = (id: string): WikiWrite => ({ changesetId: id, state: seed.wikiWritesPending === true ? "pending" : "committed", replayed: false, refusals: [] });
  /** Applies a write once per key, and answers a repeat of the key as the server does. */
  const write = (key: string, run: () => ApiResult<WikiWrite>): ApiResult<WikiWrite> => {
    writeKeys.push(key);
    const prior = replays.get(key);
    if (prior !== undefined) return ok({ ...prior, replayed: true });
    const result = run();
    if (result.ok) replays.set(key, result.value);
    if (losing) {
      losing = false;
      return failed(UNREACHABLE);
    }
    return result;
  };

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

    writeKeys,
    otherSessionEdits: (id, body) => {
      wiki.get(id)?.bodies.push(body);
    },
    wikiBody: (id) => wiki.get(id)?.bodies.at(-1),
    loseNextAnswer: () => {
      losing = true;
    },
    listWikiPages: (_vault, cursor) =>
      answer("listWikiPages", () => {
        const all = [...wiki].map(([id, p]) => wikiSummary(id, p));
        const start = cursor === null ? 0 : Number(cursor);
        const end = start + wikiPageSize;
        return ok<Page<WikiPageSummary>>({ items: all.slice(start, end), nextCursor: end < all.length ? String(end) : null });
      }),
    getWikiPage: (_vault, id, revision) =>
      answer("getWikiPage", () => {
        const p = wiki.get(id);
        if (p === undefined || (revision !== null && (revision < 1 || revision > p.bodies.length))) return failed<WikiPage>(notFound);
        return ok(wikiView(id, p, revision ?? p.bodies.length));
      }),
    resolveWikiSlug: (_vault, slug) =>
      answer("resolveWikiSlug", () => {
        const hit = [...wiki].find(([, p]) => p.slug === slug);
        return hit === undefined ? failed(notFound) : ok({ slug, pageId: hit[0], currentSlug: slug, redirected: false });
      }),
    listWikiBacklinks: () => answer("listWikiBacklinks", () => ok({ items: [], nextCursor: null })),
    listWikiLinks: () => answer("listWikiLinks", () => ok({ items: [], nextCursor: null })),
    listWikiUnresolved: () => answer("listWikiUnresolved", () => ok({ items: [], nextCursor: null })),
    listWikiRevisions: (_vault, id) =>
      answer("listWikiRevisions", () => {
        const p = wiki.get(id);
        if (p === undefined) return failed<Page<WikiRevision>>(notFound);
        return ok<Page<WikiRevision>>({
          items: p.bodies.map((_, i) => ({ revision: i + 1, title: p.title, slug: p.slug, creator: "pat", changesetId: `cs-${String(i + 1)}`, createdAt: epochMs(i + 1), restoredFrom: null })),
          nextCursor: null,
        });
      }),
    getWikiDiff: (_vault, _id, from, to) => answer("getWikiDiff", () => ok({ from, to, title: null, slug: null, blocks: [] })),
    getWikiAttribution: (_vault, changesetId) => answer("getWikiAttribution", () => ok({ proposer: "pat", machine: changesetId.startsWith("agent"), extractor: null, rationale: "because", state: "committed" })),
    exportWikiPage: (_vault, id) => answer("exportWikiPage", () => {
      const p = wiki.get(id);
      return p === undefined ? failed<string>(notFound) : ok(p.bodies.at(-1) ?? "");
    }),
    createWikiPage: (_vault, input) =>
      answer("createWikiPage", () => {
        const id = `page-${String(wiki.size + 1)}`;
        let pageId = id;
        const result = write(input.idempotencyKey, () => {
          const slug = slugify(input.title) ?? "";
          if ([...wiki.values()].some((p) => p.slug === slug)) return failed(httpFailure(409, "conflict"));
          wiki.set(id, { title: input.title, slug, bodies: [input.body] });
          return ok(committed(`cs-${id}`));
        });
        if (!result.ok) return failed(result.failure);
        pageId = result.value.changesetId?.replace("cs-", "") ?? pageId;
        return ok({ pageId, write: result.value });
      }),
    editWikiPage: (_vault, id, input) =>
      answer("editWikiPage", () =>
        write(input.idempotencyKey, () => {
          const p = wiki.get(id);
          if (p === undefined) return failed(notFound);
          if (p.bodies.length !== input.baseVersion) return failed(staleFailure);
          if (seed.wikiWritesPending !== true) p.bodies.push(input.body);
          return ok(committed(`cs-${id}-${String(p.bodies.length)}`));
        }),
      ),
    renameWikiPage: (_vault, id, input) =>
      answer("renameWikiPage", () =>
        write(input.idempotencyKey, () => {
          const p = wiki.get(id);
          if (p === undefined) return failed(notFound);
          if (p.bodies.length !== input.baseVersion) return failed(staleFailure);
          p.title = input.title;
          p.slug = input.slug ?? slugify(input.title) ?? p.slug;
          p.bodies.push(p.bodies.at(-1) ?? "");
          return ok(committed(`cs-rename-${id}`));
        }),
      ),
    revertWikiPage: (_vault, id, input) =>
      answer("revertWikiPage", () =>
        write(input.idempotencyKey, () => {
          const p = wiki.get(id);
          if (p === undefined) return failed(notFound);
          if (p.bodies.length !== input.baseVersion) return failed(staleFailure);
          p.bodies.push(p.bodies[input.revision - 1] ?? "");
          return ok(committed(`cs-revert-${id}`));
        }),
      ),
    deleteWikiPage: (_vault, id, input) =>
      answer("deleteWikiPage", () =>
        write(input.idempotencyKey, () => {
          const p = wiki.get(id);
          if (p === undefined) return failed(notFound);
          if (p.bodies.length !== input.baseVersion) return failed(staleFailure);
          return ok({ changesetId: `cs-delete-${id}`, state: "pending", replayed: false, refusals: [] });
        }),
      ),
    importWikiPage: (_vault, input) =>
      answer("importWikiPage", () => {
        const id = `page-${String(wiki.size + 1)}`;
        const result = write(input.idempotencyKey, () => {
          const title = /^# (.+)$/m.exec(input.markdown)?.[1];
          if (title === undefined) return failed(httpFailure(400, "invalid", "a page's file needs a title"));
          wiki.set(id, { title, slug: slugify(title) ?? "", bodies: [input.markdown] });
          return ok(committed(`cs-${id}`));
        });
        return result.ok ? ok({ pageId: id, created: true, write: result.value }) : failed(result.failure);
      }),
    searchClaimIds: () => answer("searchClaimIds", () => ok(seed.claimHits ?? [])),
    readClaim: (_vault, id) =>
      answer("readClaim", () => {
        const known = seed.claims?.[id];
        return known === undefined ? failed(notFound) : ok(known);
      }),
  };
  return api;
};
