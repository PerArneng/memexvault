import { vaultId } from "@memex/types";
import type { ActivityPage, DurableFrame } from "@memex/types";
import { describe, expect, it } from "vitest";
import { createActivityWatch, MAX_PENDING_FRAMES, MAX_WAITING_EVENTS, MIN_SNAPSHOT_GAP_MS } from "./activity-watch";
import { failed, ok } from "./api";
import type { ApiResult } from "./api";
import { documentActivity, scriptedFeed } from "./ingest-testing";
import { manualClock, UNREACHABLE } from "./testing";

const vault = vaultId("v");
const durable = (seq: number, over: Partial<DurableFrame> = {}): DurableFrame => ({ kind: "durable", seq, timeMs: 1, eventKind: "state.recorded", sources: [], jobs: [], renditions: [], extractions: [], unplaced: false, ...over });
const settle = async () => {
  for (let i = 0; i < 10; i += 1) await Promise.resolve();
};

const setup = () => {
  const clock = manualClock(10_000);
  const feed = scriptedFeed();
  const answers: { resolve: (r: ApiResult<ActivityPage>) => void; cursor: string | null }[] = [];
  const page = (_v: unknown, cursor: string | null) => new Promise<ApiResult<ActivityPage>>((resolve) => {
    answers.push({ resolve, cursor });
  });
  const watch = createActivityWatch({ vault, clock, feed, page });
  const answer = async (head: number, ids: readonly string[], nextCursor: string | null = null) => {
    answers.shift()?.resolve(ok({ head, items: ids.map((id) => documentActivity(id)), nextCursor }));
    await settle();
  };
  return { clock, feed, watch, answers, answer };
};

describe("createActivityWatch", () => {
  it("follows from the first snapshot's position, and only after it", async () => {
    const { feed, watch, answer } = setup();
    expect(feed.opened).toEqual([]);
    await answer(7, ["a"]);
    expect(feed.opened).toEqual([7]);
    expect(watch.state().seq).toBe(7);
    expect(watch.state().documents.has("a")).toBe(true);
  });

  it("reads every page, and takes the first page's position", async () => {
    const { feed, answers, answer, watch } = setup();
    await answer(3, ["a"], "next");
    expect(answers[0]?.cursor).toBe("next");
    await answer(9, ["b"]);
    expect(watch.state().seq).toBe(3);
    expect(watch.state().documents.size).toBe(2);
    expect(feed.opened).toEqual([3]);
  });

  it("applies frames only on a tick, in one batch with one notification", async () => {
    const { feed, watch, answer } = setup();
    await answer(0, ["a"]);
    let notified = 0;
    watch.subscribe(() => {
      notified += 1;
    });
    for (let seq = 1; seq <= 100; seq += 1) feed.emit({ type: "frame", frame: durable(seq) });
    expect(watch.state().seq).toBe(0);
    watch.tick();
    expect(watch.state().seq).toBe(100);
    expect(notified).toBe(1);
    watch.tick();
    expect(notified).toBe(1);
  });

  it("a resync fetches a new snapshot and replays what came during it", async () => {
    const { clock, feed, watch, answers, answer } = setup();
    await answer(0, ["a"]);
    clock.advance(MIN_SNAPSHOT_GAP_MS);
    feed.emit({ type: "frame", frame: { kind: "resync", head: 5, reason: "overflow" } });
    watch.tick();
    watch.tick();
    expect(answers.length).toBe(1);
    feed.emit({ type: "frame", frame: durable(6, { sources: [{ id: "during", logicalId: null, versionNo: 1 }] }) });
    await answer(5, ["a"]);
    watch.tick();
    expect(watch.state().documents.has("during")).toBe(true);
    expect(watch.state().seq).toBe(6);
    expect(watch.state().needsSnapshot).toBe(false);
    expect(feed.opened).toEqual([0]);
  });

  it("a frame seen before the snapshot's position isn't applied twice", async () => {
    const { clock, feed, watch, answer } = setup();
    await answer(0, ["a"]);
    clock.advance(MIN_SNAPSHOT_GAP_MS);
    watch.refresh();
    watch.tick();
    feed.emit({ type: "frame", frame: durable(1) });
    await answer(1, ["a", "b"]);
    watch.tick();
    expect(watch.state().seq).toBe(1);
    expect(watch.state().documents.size).toBe(2);
  });

  it("spaces out snapshots by what they cost, however much is happening", async () => {
    const { clock, feed, watch, answers, answer } = setup();
    await answer(0, ["a"]);
    clock.advance(MIN_SNAPSHOT_GAP_MS);
    feed.emit({ type: "frame", frame: durable(1, { unplaced: true }) });
    watch.tick();
    expect(answers.length).toBe(1);
    clock.advance(2000);
    await answer(1, ["a"]);
    feed.emit({ type: "frame", frame: durable(2, { unplaced: true }) });
    clock.advance(9000);
    watch.tick();
    expect(answers.length).toBe(0);
    clock.advance(1000);
    watch.tick();
    expect(answers.length).toBe(1);
  });

  it("a failed first snapshot reports it, tries again and opens nothing until it works", async () => {
    const clock = manualClock(10_000);
    const feed = scriptedFeed();
    let calls = 0;
    const watch = createActivityWatch({ vault, clock, feed, page: () => {
      calls += 1;
      return Promise.resolve(calls === 1 ? failed(UNREACHABLE) : ok({ head: 2, items: [], nextCursor: null }));
    } });
    await settle();
    expect(watch.failure()).toEqual(UNREACHABLE);
    expect(feed.opened).toEqual([]);
    watch.tick();
    expect(calls).toBe(1);
    clock.advance(5000);
    watch.tick();
    await settle();
    expect(watch.failure()).toBeNull();
    expect(feed.opened).toEqual([2]);
  });

  it("a snapshot that takes too long to replay asks for another", async () => {
    const { clock, feed, watch, answers, answer } = setup();
    await answer(0, ["a"]);
    clock.advance(MIN_SNAPSHOT_GAP_MS);
    watch.refresh();
    watch.tick();
    for (let seq = 1; seq <= MAX_PENDING_FRAMES + 1; seq += 1) feed.emit({ type: "frame", frame: durable(seq) });
    await answer(0, ["a"]);
    clock.advance(MIN_SNAPSHOT_GAP_MS);
    watch.tick();
    expect(answers.length).toBe(1);
  });

  it("a lost connection and its return show, and bring a new snapshot", async () => {
    const { clock, feed, watch, answers, answer } = setup();
    await answer(0, ["a"]);
    feed.emit({ type: "frame", frame: { kind: "hello", head: 0, resumed: true } });
    watch.tick();
    feed.emit({ type: "lost" });
    watch.tick();
    expect(watch.state().connection.status).toBe("reconnecting");
    expect(answers.length).toBe(0);
    clock.advance(MIN_SNAPSHOT_GAP_MS);
    feed.emit({ type: "frame", frame: { kind: "hello", head: 0, resumed: true } });
    watch.tick();
    expect(watch.state().connection.status).toBe("live");
    expect(answers.length).toBe(1);
  });

  it("a stream ended by the server asks again, which is how the session learns it's over", async () => {
    const { clock, feed, watch, answers, answer } = setup();
    await answer(0, ["a"]);
    clock.advance(MIN_SNAPSHOT_GAP_MS);
    feed.emit({ type: "ended", reason: "auth_ended", failure: null });
    watch.tick();
    expect(watch.state().connection).toEqual({ status: "ended", reason: "auth_ended" });
    expect(answers.length).toBe(1);
  });

  it("drops what has waited too long for a tick, and the jump asks for a snapshot", async () => {
    const { feed, watch, answer } = setup();
    await answer(0, ["a"]);
    for (let seq = 1; seq <= MAX_WAITING_EVENTS + 5; seq += 1) feed.emit({ type: "frame", frame: durable(seq) });
    watch.tick();
    expect(watch.state().seq).toBe(MAX_WAITING_EVENTS + 5);
    expect(watch.state().needsSnapshot).toBe(true);
  });

  it("stop closes the stream, and a late answer changes nothing", async () => {
    const { feed, watch, answer } = setup();
    await answer(0, ["a"]);
    watch.stop();
    expect(feed.stopped()).toBe(1);
    feed.emit({ type: "lost" });
    watch.tick();
    expect(watch.state().connection.status).toBe("connecting");
  });
});
