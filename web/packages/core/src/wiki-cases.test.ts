import { epochMs, vaultId } from "@memex/types";
import type { WikiClaim } from "@memex/types";
import { describe, expect, it } from "vitest";
import { createEngine } from "./engine";
import { DEFAULT_SETTINGS } from "./settings";
import { countingIds, fakeApi, httpFailure, identityWith, manualClock, memoryDraftStorage, memoryPreferences, UNREACHABLE } from "./testing";
import type { FakeApiSeed } from "./testing";

const v = vaultId("v");
const pat = identityWith([{ vault: v, scope: "propose" }]);

const setup = (seed: FakeApiSeed = {}) => {
  const clock = manualClock(1000);
  const api = fakeApi({ session: pat, ...seed });
  const drafts = memoryDraftStorage();
  const engine = createEngine({ clock, settings: DEFAULT_SETTINGS, api, preferences: memoryPreferences(), drafts, ids: countingIds() });
  return { api, clock, drafts, engine };
};

const NOTE = { rationale: "why" };

const created = async (engine: ReturnType<typeof setup>["engine"], body = "one\n") => {
  const out = await engine.createWikiPage(v, { title: "Pump", body, ...NOTE });
  if (out.pageId === undefined) throw new Error("no page");
  return out.pageId;
};

const claim = (id: string): WikiClaim => ({ claimId: id, status: "current", currentClaimId: id, subject: "pump", predicate: "opened", value: "1912", epistemic: "asserted", citations: [] });

describe("saving a page", () => {
  it("saves from the current version", async () => {
    const { api, engine } = setup();
    const id = await created(engine);
    expect((await engine.saveWikiPage(v, id, { baseVersion: 1, body: "two\n", ...NOTE })).kind).toBe("saved");
    expect(api.wikiBody(id)).toBe("two\n");
  });

  it("never overwrites from a stale version: it reports both sides and the base", async () => {
    const { api, engine } = setup();
    const id = await created(engine);
    api.otherSessionEdits(id, "theirs\n");
    const out = await engine.saveWikiPage(v, id, { baseVersion: 1, body: "mine\n", ...NOTE });
    expect(api.wikiBody(id)).toBe("theirs\n");
    if (out.kind !== "conflict") throw new Error(out.kind);
    expect(out.mine).toBe("mine\n");
    expect(out.theirs.body).toBe("theirs\n");
    expect(out.theirs.currentRevision).toBe(2);
    expect(out.base?.body).toBe("one\n");
  });

  it("still shows the conflict when the base revision can't be read", async () => {
    const { api, engine } = setup();
    const id = await created(engine);
    api.otherSessionEdits(id, "theirs\n");
    const original = api.getWikiPage.bind(api);
    api.getWikiPage = (vault, page, revision) => (revision === null ? original(vault, page, null) : Promise.resolve({ ok: false, failure: httpFailure(404, "not_found") }));
    const out = await engine.saveWikiPage(v, id, { baseVersion: 1, body: "mine\n", ...NOTE });
    expect(out.kind === "conflict" && out.base).toBeNull();
  });

  it("counts a conflict with identical text as saved, as there's nothing to choose", async () => {
    const { api, engine } = setup();
    const id = await created(engine);
    api.otherSessionEdits(id, "same\n");
    expect((await engine.saveWikiPage(v, id, { baseVersion: 1, body: "same\n", ...NOTE })).kind).toBe("saved");
  });

  it("reports review, and leaves the page alone, when governance holds the edit", async () => {
    const { api, engine } = setup({ wikiWritesPending: true });
    const id = await created(engine);
    expect((await engine.saveWikiPage(v, id, { baseVersion: 1, body: "two\n", ...NOTE })).kind).toBe("pending");
    expect(api.wikiBody(id)).toBe("one\n");
  });

  it("retries a write whose answer got lost under the same key, so it doesn't conflict with itself", async () => {
    const { api, engine } = setup();
    const id = await created(engine);
    api.loseNextAnswer();
    const first = await engine.saveWikiPage(v, id, { baseVersion: 1, body: "two\n", ...NOTE });
    expect(first.kind).toBe("failed");
    expect(api.wikiBody(id)).toBe("two\n");
    const retry = await engine.saveWikiPage(v, id, { baseVersion: 1, body: "two\n", ...NOTE });
    expect(retry.kind).toBe("saved");
    const [, lost, again] = api.writeKeys;
    expect(again).toBe(lost);
    expect(api.wikiBody(id)).toBe("two\n");
  });

  it("uses a new key once a write has had an answer, and for different text", async () => {
    const { api, engine } = setup();
    const id = await created(engine);
    await engine.saveWikiPage(v, id, { baseVersion: 1, body: "two\n", ...NOTE });
    await engine.saveWikiPage(v, id, { baseVersion: 2, body: "three\n", ...NOTE });
    api.failNext(httpFailure(500, "internal"));
    await engine.saveWikiPage(v, id, { baseVersion: 3, body: "four\n", ...NOTE });
    await engine.saveWikiPage(v, id, { baseVersion: 3, body: "four\n", ...NOTE });
    expect(new Set(api.writeKeys).size).toBe(api.writeKeys.length);
  });

  it("turns a server error into data", async () => {
    const { api, engine } = setup();
    const id = await created(engine);
    api.failNext(httpFailure(500, "internal"));
    const out = await engine.saveWikiPage(v, id, { baseVersion: 1, body: "x", ...NOTE });
    expect(out.kind === "failed" && out.failure.kind).toBe("http");
  });

  it("reports no answer as offline, as any call does", async () => {
    const { api, engine } = setup();
    const id = await created(engine);
    api.failNext(UNREACHABLE);
    expect((await engine.saveWikiPage(v, id, { baseVersion: 1, body: "x", ...NOTE })).kind).toBe("failed");
    expect(engine.connectivity()).toEqual({ status: "online", failures: 1 });
  });
});

describe("other writes", () => {
  it("renames, reverts and deletes from the current version", async () => {
    const { api, engine } = setup();
    const id = await created(engine);
    expect((await engine.renameWikiPage(v, id, { baseVersion: 1, title: "New", slug: null, ...NOTE })).kind).toBe("saved");
    await engine.saveWikiPage(v, id, { baseVersion: 2, body: "later\n", ...NOTE });
    expect((await engine.revertWikiPage(v, id, { baseVersion: 3, revision: 1, ...NOTE })).kind).toBe("saved");
    expect(api.wikiBody(id)).toBe("one\n");
    // A delete always waits for review.
    expect((await engine.deleteWikiPage(v, id, { baseVersion: 4, ...NOTE })).kind).toBe("pending");
  });

  it("does nothing from a stale version", async () => {
    const { api, engine } = setup();
    const id = await created(engine);
    api.otherSessionEdits(id, "theirs\n");
    expect((await engine.renameWikiPage(v, id, { baseVersion: 1, title: "New", slug: null, ...NOTE })).kind).toBe("stale");
    expect((await engine.revertWikiPage(v, id, { baseVersion: 1, revision: 1, ...NOTE })).kind).toBe("stale");
    expect((await engine.deleteWikiPage(v, id, { baseVersion: 1, ...NOTE })).kind).toBe("stale");
    expect(api.wikiBody(id)).toBe("theirs\n");
  });

  it("imports, and passes on why a file was refused", async () => {
    const { engine } = setup();
    const out = await engine.importWikiPage(v, { markdown: "# T\n", baseVersion: null, ...NOTE });
    expect(out.kind).toBe("saved");
    expect(out.pageId).toBeDefined();
    const bad = await engine.importWikiPage(v, { markdown: "no title", baseVersion: null, ...NOTE });
    expect(bad.kind === "failed" && bad.failure.message).toBe("a page's file needs a title");
  });
});

describe("listing", () => {
  it("follows cursors", async () => {
    const { engine } = setup({ wikiPageSize: 2 });
    for (const title of ["a", "b", "c", "d", "e"]) await engine.createWikiPage(v, { title, body: "x", ...NOTE });
    const list = await engine.listWikiPages(v);
    expect(list.ok && list.value.pages.map((p) => p.title)).toEqual(["a", "b", "c", "d", "e"]);
    expect(list.ok && list.value.truncated).toBe(false);
  });

  it("stops at its limit and says so, when a server's cursors never end", async () => {
    const { api, engine } = setup();
    api.listWikiPages = () => Promise.resolve({ ok: true, value: { items: [{ id: "x", title: "x", slug: "x", redirects: [], version: 1, creator: "pat", updatedAt: epochMs(0) }], nextCursor: "again" } });
    const list = await engine.listWikiPages(v);
    expect(list.ok && list.value.truncated).toBe(true);
    expect(list.ok && list.value.pages).toHaveLength(50);
  });

  it("passes on a failure part way", async () => {
    const { api, engine } = setup();
    api.failNext(httpFailure(403, "forbidden"));
    expect((await engine.listWikiPages(v)).ok).toBe(false);
  });
});

describe("lookupClaim, for a claim a draft names", () => {
  it("reads a current claim", async () => {
    const { engine } = setup({ claims: { a: { claim: claim("a"), supersededBy: null } } });
    expect(await engine.lookupClaim(v, "a")).toEqual({ ok: true, value: claim("a") });
  });

  it("follows replacements to what stands, keeping the id the page named", async () => {
    const { engine } = setup({ claims: { a: { claim: claim("a"), supersededBy: "b" }, b: { claim: claim("b"), supersededBy: "c" }, c: { claim: { ...claim("c"), value: "1913" }, supersededBy: null } } });
    const got = await engine.lookupClaim(v, "a");
    expect(got.ok && got.value).toEqual({ ...claim("a"), status: "superseded", currentClaimId: "c", value: "1913" });
  });

  it("calls an unknown id or a dead end missing, not an error", async () => {
    const { engine } = setup({ claims: { a: { claim: claim("a"), supersededBy: "gone" } } });
    for (const id of ["nope", "a"]) {
      const got = await engine.lookupClaim(v, id);
      expect(got.ok && got.value.status).toBe("missing");
      expect(got.ok && got.value.claimId).toBe(id);
    }
  });

  it("ends on a cycle of replacements", async () => {
    const { engine } = setup({ claims: { a: { claim: claim("a"), supersededBy: "b" }, b: { claim: claim("b"), supersededBy: "a" } } });
    const got = await engine.lookupClaim(v, "a");
    expect(got.ok && got.value.status).toBe("missing");
  });

  it("passes on a real failure", async () => {
    const { api, engine } = setup();
    api.failNext(httpFailure(500, "internal"));
    expect((await engine.lookupClaim(v, "a")).ok).toBe(false);
  });
});

describe("suggestClaims", () => {
  it("labels hits and leaves out replaced or missing ones", async () => {
    const { engine } = setup({ claimHits: ["a", "old", "gone"], claims: { a: { claim: claim("a"), supersededBy: null }, old: { claim: claim("old"), supersededBy: "a" } } });
    expect(await engine.suggestClaims(v, "pump")).toEqual({ ok: true, value: [{ id: "a", label: "pump opened 1912" }] });
  });
});

describe("draftKeeper", () => {
  it("keeps a draft per person, so another sign-in on this browser doesn't see it", async () => {
    const { clock, drafts, engine } = setup();
    await engine.restoreSession();
    const keeper = engine.draftKeeper(v, "p");
    keeper.edit({ body: "typed", title: null, baseVersion: 1, pristineBody: "", pristineTitle: null });
    keeper.flush();
    const other = createEngine({ clock, settings: DEFAULT_SETTINGS, api: fakeApi({ session: identityWith([], true) }), preferences: memoryPreferences(), drafts, ids: countingIds() });
    expect(other.draftKeeper(v, "p").load()).toBeNull();
    expect(engine.draftKeeper(v, "p").load()?.body).toBe("typed");
  });
});
