import { vaultId } from "@memex/types";
import { describe, expect, it, vi } from "vitest";
import { createEngine } from "./engine";
import type { QueueFeedEvent } from "./review-cases";
import { DEFAULT_SETTINGS } from "./settings";
import { changesetWith, combinationWith, countingIds, fakeApi, detailWith, httpFailure, manualClock, memoryDraftStorage, memoryPreferences } from "./testing";
import type { FakeApiSeed } from "./testing";

const vault = vaultId("v");
const start = (review: FakeApiSeed["review"], queueFeed?: Parameters<typeof createEngine>[0]["queueFeed"]) => {
  const api = fakeApi({ review: review ?? {} });
  const engine = createEngine({ clock: manualClock(), settings: DEFAULT_SETTINGS, api, preferences: memoryPreferences(), drafts: memoryDraftStorage(), ids: countingIds(), ...(queueFeed === undefined ? {} : { queueFeed }) });
  return { api, engine };
};
const item = (id: string) => detailWith(changesetWith({ id }));

describe("deciding", () => {
  it("approves, with the reason and who on record", async () => {
    const { engine } = start({ items: [item("a")] });
    const out = await engine.decide(vault, { kind: "approve", id: "a", reason: "fine" });
    expect(out).toMatchObject({ kind: "done", changeset: { state: "committed", decidedBy: "pat", decisionReason: "fine" } });
  });
  it("rejects, and refuses to with no reason", async () => {
    const { engine } = start({ items: [item("a")] });
    expect((await engine.decide(vault, { kind: "reject", id: "a", reason: " " })).kind).toBe("invalid");
    expect(await engine.decide(vault, { kind: "reject", id: "a", reason: "wrong" })).toMatchObject({ kind: "done", changeset: { state: "rejected" } });
  });
  it("an edit keeps what was proposed", async () => {
    const original = [{ op: "add_assertion" }];
    const { engine } = start({ items: [detailWith(changesetWith({ id: "a", ops: original }))] });
    const out = await engine.decide(vault, { kind: "edit_approve", id: "a", ops: [{ op: "edit_page" }], reason: "better" });
    expect(out).toMatchObject({ kind: "done", changeset: { ops: [{ op: "edit_page" }], originalOps: original } });
  });
  it("a second person gets a conflict naming the first, and nothing changes", async () => {
    const { engine } = start({ items: [detailWith(changesetWith({ id: "a", state: "committed", decidedBy: "sam", decisionReason: "yes" }))] });
    expect(await engine.decide(vault, { kind: "reject", id: "a", reason: "no" })).toEqual({ kind: "conflict", message: "sam already decided this" });
    const still = await engine.getReviewItem(vault, "a");
    expect(still.ok && still.value.changeset.decidedBy).toBe("sam");
  });
  it("the same person deciding again is the same decision", async () => {
    const { engine } = start({ items: [item("a")] });
    await engine.decide(vault, { kind: "approve", id: "a", reason: "fine" });
    expect((await engine.decide(vault, { kind: "approve", id: "a", reason: "fine" })).kind).toBe("done");
  });
  it("an agent's session is refused as not allowed", async () => {
    const { api, engine } = start({ items: [item("a")] });
    api.failNext(httpFailure(403, "agent_session", "an agent can't"));
    expect(await engine.decide(vault, { kind: "approve", id: "a", reason: null })).toEqual({ kind: "not_allowed", message: "an agent can't" });
  });
  it("no answer isn't reported as a refusal", async () => {
    const { api, engine } = start({ items: [item("a")] });
    api.down = true;
    expect((await engine.decide(vault, { kind: "approve", id: "a", reason: null })).kind).toBe("failed");
  });
});

describe("batches", () => {
  it("each decision stands alone, so one conflict leaves the others", async () => {
    const { engine } = start({ items: [item("a"), detailWith(changesetWith({ id: "b", state: "committed", decidedBy: "sam" })), item("c")] });
    const out = await engine.decideBatch(vault, ["a", "b", "c"].map((id) => ({ kind: "approve", id, reason: "r" })));
    expect(out.ok && out.value.map((r) => r.errorCode)).toEqual([null, "conflict", null]);
  });
  it("sends a long selection in pieces the server accepts", async () => {
    const items = Array.from({ length: 450 }, (_, i) => item(`i${String(i)}`));
    const { api, engine } = start({ items });
    const out = await engine.decideBatch(vault, items.map((d) => ({ kind: "approve", id: d.changeset.id, reason: "r" })));
    expect(out.ok && out.value.every((r) => r.errorCode === null)).toBe(true);
    expect(api.calls.filter((c) => c === "decideBatch")).toHaveLength(3);
  });
  it("a failed piece after a good one says what wasn't sent, rather than hiding what was", async () => {
    const items = Array.from({ length: 201 }, (_, i) => item(`i${String(i)}`));
    const { api, engine } = start({ items });
    const decisions = items.map((d) => ({ kind: "approve" as const, id: d.changeset.id, reason: "r" }));
    const original = api.decideBatch.bind(api);
    let calls = 0;
    api.decideBatch = (v, d) => {
      calls += 1;
      if (calls === 2) api.down = true;
      return original(v, d);
    };
    const out = await engine.decideBatch(vault, decisions);
    expect(out.ok && out.value.filter((r) => r.errorCode === null)).toHaveLength(200);
    expect(out.ok && out.value[200]?.errorCode).toBe("not_sent");
  });
  it("a first piece failing is a failure", async () => {
    const { api, engine } = start({ items: [item("a")] });
    api.down = true;
    expect((await engine.decideBatch(vault, [{ kind: "approve", id: "a", reason: "r" }])).ok).toBe(false);
  });
});

describe("reverting and the table", () => {
  it("reverts what's committed, and refuses what isn't", async () => {
    const { engine } = start({ items: [detailWith(changesetWith({ id: "a", state: "committed" })), item("b")] });
    expect(await engine.revertChangeset(vault, "a", "undo")).toMatchObject({ kind: "done", changeset: { reverts: "a" } });
    expect((await engine.revertChangeset(vault, "b", "undo")).kind).toBe("conflict");
  });
  it("reads every page of the table, with its threshold", async () => {
    const { api, engine } = start({ autoCommit: { threshold: 0.9, entries: [combinationWith({ id: "x", on: true })] } });
    const original = api.listAutoCommit.bind(api);
    let page = 0;
    api.listAutoCommit = async (v, c) => {
      page += 1;
      const out = await original(v, c);
      return out.ok && page === 1 ? { ok: true, value: { ...out.value, nextCursor: "more" } } : out;
    };
    const table = await engine.getAutoCommitTable(vault);
    expect(table.ok && table.value.threshold).toBe(0.9);
    expect(table.ok && table.value.entries).toHaveLength(2);
  });
  it("a turned off combination says who, and starts its counts again", async () => {
    const { engine } = start({ autoCommit: { threshold: 0.95, entries: [combinationWith({ id: "x", on: true, correct: 80 })] } });
    const out = await engine.disableAutoCommit(vault, "x", "bad run");
    expect(out.ok && out.value).toMatchObject({ on: false, correct: 0, history: [{ kind: "disabled", by: "pat", reason: "bad run", correct: 80 }] });
  });
  it("a failure partway through the table is a failure", async () => {
    const { api, engine } = start({});
    api.failNext(httpFailure(403, "forbidden"));
    expect((await engine.getAutoCommitTable(vault)).ok).toBe(false);
  });
});

describe("following the queue", () => {
  it("passes on what the feed says, and stops when asked", () => {
    const stop = vi.fn();
    let send: (event: QueueFeedEvent) => void = () => undefined;
    const { engine } = start({}, { follow: (_v, listener) => { send = listener; return stop; } });
    const seen: QueueFeedEvent[] = [];
    const end = engine.followQueue(vault, (e) => seen.push(e));
    send({ kind: "changed" });
    expect(seen).toEqual([{ kind: "changed" }]);
    end();
    expect(stop).toHaveBeenCalled();
  });
  it("with no feed, says so at once, so the screen reads by itself", () => {
    const { engine } = start({});
    const seen: QueueFeedEvent[] = [];
    engine.followQueue(vault, (e) => seen.push(e))();
    expect(seen).toEqual([{ kind: "closed" }]);
  });
});
