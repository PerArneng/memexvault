import { vaultId } from "@memex/types";
import { describe, expect, it } from "vitest";
import { createEngine } from "./engine";
import { ingestPath, parseIngestRoute } from "./ingest-cases";
import { documentActivity, jobNamed, scriptedFeed, scriptedUploader } from "./ingest-testing";
import { DEFAULT_SETTINGS } from "./settings";
import { countingIds, fakeApi, identityWith, manualClock, memoryDraftStorage, memoryPreferences } from "./testing";

const vault = vaultId("v");
const start = (ingest: Parameters<typeof fakeApi>[0]) => {
  const feed = scriptedFeed();
  const engine = createEngine({ clock: manualClock(), settings: DEFAULT_SETTINGS, api: fakeApi(ingest), preferences: memoryPreferences(), drafts: memoryDraftStorage(), ids: countingIds(), activity: feed });
  return { engine, feed };
};

describe("the engine's ingest cases", () => {
  it("watches a vault through the feed it was given", async () => {
    const { engine, feed } = start({ ingest: { head: 4, documents: [documentActivity("a")] } });
    const watch = engine.watchActivity(vault);
    for (let i = 0; i < 10; i += 1) await Promise.resolve();
    expect(feed.opened).toEqual([4]);
    expect(watch.state().documents.has("a")).toBe(true);
    watch.stop();
  });

  it("finds one document's jobs among all of them, oldest first", async () => {
    const { engine } = start({ ingest: { jobs: [jobNamed("x", { kind: "extract", sourceId: "s", createdAtMs: 5 }), jobNamed("r", { sourceId: "s", createdAtMs: 1 }), jobNamed("other", { sourceId: "t" })] } });
    const result = await engine.listDocumentJobs(vault, "s", null);
    expect(result.ok && result.value.jobs.map((j) => j.id)).toEqual(["r", "x"]);
  });

  it("a bulk retry carries on past a failure and reports both", async () => {
    const { engine } = start({ ingest: { jobs: [jobNamed("a", { state: "dead" }), jobNamed("b", { state: "dead" })] } });
    const outcome = await engine.retryJobs(vault, ["a", "missing", "b"]);
    expect(outcome.retried).toEqual(["a", "b"]);
    expect(outcome.failed.map((f) => f.id)).toEqual(["missing"]);
    const dead = await engine.listJobsInState(vault, "dead");
    expect(dead.ok && dead.value.jobs).toEqual([]);
  });

  it("an uploader that was never given reports no answer rather than throwing", async () => {
    const { engine } = start({});
    const queue = engine.uploadQueue(vault, { maxBytes: 100, allowedMediaTypes: ["text/plain"] });
    queue.add([{ name: "a.txt", size: 1, type: "text/plain" }], { trustTier: "unverified", language: null });
    for (let i = 0; i < 10; i += 1) await Promise.resolve();
    expect(queue.state().items[0]?.state.kind).toBe("failed");
  });
});

describe("the upload queue's life", () => {
  const limits = { maxBytes: 100, allowedMediaTypes: ["text/plain"] };
  const setup = async () => {
    const uploader = scriptedUploader();
    const engine = createEngine({ clock: manualClock(), settings: DEFAULT_SETTINGS, api: fakeApi({ session: identityWith([]) }), preferences: memoryPreferences(), drafts: memoryDraftStorage(), ids: countingIds(), uploader });
    await engine.restoreSession();
    return { engine, uploader };
  };
  it("is the same queue however often a screen asks, so moving away doesn't lose an upload", async () => {
    const { engine } = await setup();
    expect(engine.uploadQueue(vault, limits)).toBe(engine.uploadQueue(vault, limits));
  });
  it("aborts when the session ends, and the next user starts with an empty one", async () => {
    const { engine, uploader } = await setup();
    const queue = engine.uploadQueue(vault, limits);
    queue.add([{ name: "a.txt", size: 1, type: "text/plain" }], { trustTier: "unverified", language: null });
    await engine.signOut();
    expect(uploader.aborted).toEqual(["a.txt"]);
    expect(engine.uploadQueue(vault, limits)).not.toBe(queue);
  });
});

describe("ingest addresses", () => {
  const parse = (rest: readonly string[], search = "") => {
    const pairs = new Map(search.split("&").map((pair) => {
      const [name = "", value = ""] = pair.split("=");
      return [name, decodeURIComponent(value)] as const;
    }));
    return parseIngestRoute(rest, (n) => pairs.get(n) ?? null);
  };
  it("reads the board, a document and the failed list, and round-trips them", () => {
    for (const route of [
      { kind: "board", stage: "ocr", status: "failed", text: "a b" },
      { kind: "document", id: "s 1" },
      { kind: "failed", state: "cancelled" },
    ] as const) {
      const [path = "", search = ""] = ingestPath(vault, route).split("?");
      const rest = path.split("/").slice(4).map(decodeURIComponent);
      expect(parse(rest, search)).toEqual(route);
    }
    expect(parse(["-", "failed"])).toEqual({ kind: "failed", state: "dead" });
  });
  it("lands anything else on unknown", () => {
    expect(parse(["a", "b"]).kind).toBe("unknown");
    expect(parse(["-", "nope"]).kind).toBe("unknown");
  });
});
