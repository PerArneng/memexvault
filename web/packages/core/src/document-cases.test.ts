import { epochMs, vaultId } from "@memex/types";
import type { DocumentSource } from "@memex/types";
import { describe, expect, it } from "vitest";
import { createEngine } from "./engine";
import { DEFAULT_SETTINGS } from "./settings";
import { countingIds, fakeApi, httpFailure, manualClock, memoryDraftStorage, memoryPreferences, UNREACHABLE } from "./testing";
import type { DocumentsSeed, FakeApi } from "./testing";

const V = vaultId("v");
const source = (id: string, over: Partial<DocumentSource> = {}): DocumentSource => ({
  id, name: `${id}.pdf`, sha256: "x", mediaType: "application/pdf", trustTier: "unverified", size: 1, logicalId: id, versionNo: 1, version: 4, createdBy: "p", recordedAt: epochMs(0), ...over,
});

const start = (documents: DocumentsSeed): { readonly api: FakeApi; readonly engine: ReturnType<typeof createEngine> } => {
  const api = fakeApi({ documents });
  const engine = createEngine({ clock: manualClock(), settings: DEFAULT_SETTINGS, api, preferences: memoryPreferences(), drafts: memoryDraftStorage(), ids: countingIds() });
  return { api, engine };
};

describe("listing sources", () => {
  it("follows the cursors to the end", async () => {
    const { engine, api } = start({ sources: [source("a"), source("b"), source("c")], sourcePageSize: 1 });
    const result = await engine.listSources(V);
    expect(result.ok && result.value.sources.map((s) => s.id)).toEqual(["a", "b", "c"]);
    expect(result.ok && result.value.truncated).toBe(false);
    expect(api.calls.filter((c) => c === "listSources")).toHaveLength(3);
  });

  it("says when it stopped short, rather than following cursors for ever", async () => {
    const { engine } = start({ sources: Array.from({ length: 60 }, (_, i) => source(String(i))), sourcePageSize: 1 });
    const result = await engine.listSources(V);
    expect(result.ok && result.value.truncated).toBe(true);
    expect(result.ok && result.value.sources).toHaveLength(50);
  });

  it("passes a failure on as data", async () => {
    const { engine, api } = start({});
    api.failNext(httpFailure(403, "forbidden"));
    const result = await engine.listSources(V);
    expect(result.ok).toBe(false);
  });
});

describe("addresses", () => {
  it("stay same-origin and encode what they're given", () => {
    const { engine } = start({});
    expect(engine.pageImageUrl(V, "s/1", 3, 150)).toBe("/api/v1/vaults/v/sources/s%2F1/pages/3/image?dpi=150");
    expect(engine.thumbnailUrl(V, "s", 0)).toBe("/api/v1/vaults/v/sources/s/pages/0/thumbnail");
    expect(engine.streamUrl(V, "s")).toBe("/api/v1/vaults/v/sources/s/stream");
    expect(engine.fileUrl(V, "s")).toBe("/api/v1/vaults/v/sources/s/file");
  });
});

describe("changing a source's trust tier", () => {
  const input = { baseVersion: 4, tier: "trusted", rationale: "checked" };

  it("reports a commit", async () => {
    const { engine } = start({ sources: [source("s")] });
    expect(await engine.setSourceTrust(V, "s", input)).toEqual({ kind: "saved", changesetId: "cs-trust" });
  });

  it("reports that it waits for a person", async () => {
    const { engine } = start({ sources: [source("s")], trustState: "pending" });
    expect(await engine.setSourceTrust(V, "s", input)).toEqual({ kind: "pending", changesetId: "cs-trust" });
  });

  it("reports a refusal with its reasons", async () => {
    const { engine } = start({ sources: [source("s")], trustState: "rejected" });
    expect(await engine.setSourceTrust(V, "s", input)).toEqual({ kind: "refused", reasons: ["refused by a validator"] });
  });

  it("reports that someone got there first", async () => {
    const { engine } = start({ sources: [source("s", { version: 5 })] });
    expect(await engine.setSourceTrust(V, "s", input)).toEqual({ kind: "stale" });
  });

  it("gives what a token below the scope gets from the server", async () => {
    const { engine, api } = start({ sources: [source("s")] });
    api.failNext(httpFailure(403, "forbidden"));
    const out = await engine.setSourceTrust(V, "s", input);
    expect(out.kind === "failed" && out.failure.kind === "http" && out.failure.status).toBe(403);
  });

  it("reuses its key after no answer, so a retry can't apply twice, and takes a new one after an answer", async () => {
    const { engine, api } = start({ sources: [source("s")] });
    api.failNext(UNREACHABLE);
    expect((await engine.setSourceTrust(V, "s", input)).kind).toBe("failed");
    expect((await engine.setSourceTrust(V, "s", input)).kind).toBe("saved");
    expect((await engine.setSourceTrust(V, "s", input)).kind).toBe("saved");
    expect(api.writeKeys).toHaveLength(2);
    expect(api.writeKeys[0]).not.toBe(api.writeKeys[1]);
  });
});

describe("finding what derived a segment", () => {
  const derivation = (id: string, segmentIds: readonly string[]) => ({ id, kind: "extraction", model: "m", promptHash: "h", outcome: "done", segmentIds, createdAt: epochMs(0) });

  it("looks through every page", async () => {
    const { engine } = start({ derivations: { r: [derivation("d1", ["a"]), derivation("d2", ["b"])] } });
    const found = await engine.findDerivation(V, "r", "b");
    expect(found.ok && found.value?.id).toBe("d2");
  });

  it("is nothing for a segment a person cited", async () => {
    const { engine } = start({ derivations: { r: [derivation("d1", ["a"])] } });
    const found = await engine.findDerivation(V, "r", "z");
    expect(found.ok && found.value).toBeNull();
  });
});
