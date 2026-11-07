import { vaultId } from "@memex/types";
import { describe, expect, it } from "vitest";
import { createEngine } from "./engine";
import { DEFAULT_SETTINGS } from "./settings";
import { countingIds, fakeApi, httpFailure, identityWith, manualClock, memoryDraftStorage, memoryPreferences, UNREACHABLE, vaultNamed } from "./testing";
import type { FakeApiSeed } from "./testing";

const a = vaultId("a");
const pat = identityWith([{ vault: a, scope: "read" }]);

const setup = (seed: FakeApiSeed = {}) => {
  const clock = manualClock(1000);
  const api = fakeApi({ tokens: { good: pat }, vaults: [vaultNamed("a"), vaultNamed("b")], ...seed });
  const engine = createEngine({ clock, settings: DEFAULT_SETTINGS, api, preferences: memoryPreferences(), drafts: memoryDraftStorage(), ids: countingIds() });
  return { api, clock, engine };
};

describe("createEngine", () => {
  it("measures uptime from the injected clock", () => {
    const { clock, engine } = setup();
    expect(engine.uptimeMs()).toBe(0);
    clock.advance(250);
    expect(engine.uptimeMs()).toBe(250);
    expect(engine.startedAt()).toBe(1000);
  });
});

describe("session use cases", () => {
  it("signs in with a good token, and out again", async () => {
    const { api, engine } = setup();
    expect((await engine.signIn("good")).ok).toBe(true);
    expect(engine.session()).toEqual({ status: "signed_in", identity: pat });
    expect((await engine.signOut()).ok).toBe(true);
    expect(engine.session()).toEqual({ status: "signed_out", reason: "signed_out" });
    expect((await api.whoami()).ok).toBe(false);
  });
  it("a wrong token leaves the state alone and returns the failure", async () => {
    const { engine } = setup();
    await engine.restoreSession();
    const result = await engine.signIn("bad");
    expect(result.ok).toBe(false);
    expect(engine.session()).toEqual({ status: "signed_out", reason: "none" });
  });
  it("restores an existing session", async () => {
    const { engine } = setup({ session: pat });
    await engine.restoreSession();
    expect(engine.session()).toEqual({ status: "signed_in", identity: pat });
  });
  it("any call that gets a 401 ends the session as expired", async () => {
    const { api, engine } = setup({ session: pat });
    await engine.restoreSession();
    api.endSession();
    expect((await engine.getVault(a)).ok).toBe(false);
    expect(engine.session()).toEqual({ status: "signed_out", reason: "expired" });
  });
  it("a failed sign-out keeps the session, since the cookie is still good", async () => {
    const { api, engine } = setup({ session: pat });
    await engine.restoreSession();
    api.failNext(httpFailure(500, "internal"));
    expect((await engine.signOut()).ok).toBe(false);
    expect(engine.session().status).toBe("signed_in");
  });
  it("no server at startup is unknown, and the probe that finds it rechecks the session", async () => {
    const { api, engine } = setup({ session: pat });
    api.down = true;
    await engine.restoreSession();
    expect(engine.session()).toEqual({ status: "unknown" });
    api.down = false;
    await engine.probe();
    expect(engine.session().status).toBe("signed_in");
  });
  it("a server error at startup is also unknown, not a sign-in page", async () => {
    const { api, engine } = setup({ session: pat });
    api.failNext(httpFailure(503, "unavailable"));
    await engine.restoreSession();
    expect(engine.session()).toEqual({ status: "unknown" });
  });
});

describe("vault use cases", () => {
  it("follows cursors to the end", async () => {
    const { api, engine } = setup({ session: pat, pageSize: 1 });
    const result = await engine.listVaults();
    expect(result).toMatchObject({ ok: true, value: [{ id: "a" }, { id: "b" }] });
    expect(api.calls.filter((c) => c === "listVaults")).toHaveLength(2);
  });
  it("a failure on a later page fails the whole list", async () => {
    const { api, engine } = setup({ session: pat, pageSize: 1 });
    const pending = engine.listVaults();
    api.failNext(httpFailure(500, "internal"));
    expect((await pending).ok).toBe(false);
  });
  it("stops at a limit if a server's cursors never end", async () => {
    const { api, engine } = setup({ session: pat });
    api.listVaults = async () => ({ ok: true, value: { items: [vaultNamed("x")], nextCursor: "again" } });
    const result = await engine.listVaults();
    expect(result).toMatchObject({ ok: true });
    expect(result.ok && result.value.length).toBe(50);
  });
  it("remembers the last vault, and forgets it when it's gone", () => {
    const { engine } = setup();
    engine.rememberVault(vaultId("b"));
    expect(engine.homeVault([vaultNamed("a"), vaultNamed("b")])).toBe("b");
    expect(engine.homeVault([vaultNamed("a")])).toBe("a");
  });
  it("returns a missing vault as a failure", async () => {
    const { engine } = setup({ session: pat });
    expect(await engine.getVault(vaultId("nope"))).toMatchObject({ ok: false, failure: { status: 404 } });
    expect(engine.session().status).toBe("checking");
  });
});

describe("connectivity use cases", () => {
  it("goes offline after two failed probes and recovers at one", async () => {
    const { api, clock, engine } = setup();
    await engine.probe();
    expect(engine.connectivity().status).toBe("online");
    api.down = true;
    await engine.probe();
    expect(engine.connectivity().status).toBe("online");
    clock.advance(1500);
    await engine.probe();
    expect(engine.connectivity()).toEqual({ status: "offline", since: 2500 });
    api.down = false;
    await engine.probe();
    expect(engine.connectivity().status).toBe("online");
  });
  it("a health check that answers with an error counts as a failure", async () => {
    const { api, engine } = setup();
    api.failNext(httpFailure(503, "unavailable"));
    api.failNext(httpFailure(503, "unavailable"));
    await engine.probe();
    await engine.probe();
    expect(engine.connectivity().status).toBe("offline");
  });
  it("ordinary calls that get no answer count too", async () => {
    const { api, engine } = setup({ session: pat });
    api.down = true;
    await engine.getVault(a);
    await engine.getVault(a);
    expect(engine.connectivity().status).toBe("offline");
  });
  it("a server error on an ordinary call isn't offline", async () => {
    const { api, engine } = setup({ session: pat });
    api.failNext(httpFailure(500, "internal"));
    api.failNext(httpFailure(500, "internal"));
    await engine.getVault(a);
    await engine.getVault(a);
    expect(engine.connectivity().status).toBe("online");
  });
  it("overlapping probes share one request", async () => {
    const { api, engine } = setup();
    await Promise.all([engine.probe(), engine.probe(), engine.probe()]);
    expect(api.calls.filter((c) => c === "health")).toHaveLength(1);
    await engine.probe();
    expect(api.calls.filter((c) => c === "health")).toHaveLength(2);
  });
});

describe("notices and subscribers", () => {
  it("adds and dismisses a notice with its request id", () => {
    const { engine } = setup();
    engine.reportFailure(httpFailure(500, "internal", "broke"), "read");
    const [notice] = engine.notices();
    expect(notice).toMatchObject({ title: "broke", detail: "request id: req-1" });
    engine.dismissNotice(notice?.id ?? "");
    expect(engine.notices()).toEqual([]);
  });
  it("adds nothing for what the banner already says", () => {
    const { engine } = setup();
    engine.reportFailure(UNREACHABLE, "read");
    expect(engine.notices()).toEqual([]);
  });
  it("tells subscribers about real changes only, until they stop", async () => {
    const { engine } = setup();
    let calls = 0;
    const stop = engine.subscribe(() => { calls += 1; });
    await engine.probe();
    expect(calls).toBe(0);
    engine.reportFailure(httpFailure(500, "internal"), "read");
    expect(calls).toBe(1);
    stop();
    engine.reportFailure(httpFailure(500, "internal"), "read");
    expect(calls).toBe(1);
  });
});
