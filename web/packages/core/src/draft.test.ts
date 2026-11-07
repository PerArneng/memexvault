import { epochMs, vaultId } from "@memex/types";
import { describe, expect, it } from "vitest";
import { createDraftKeeper, decodeDraft, DRAFT_MAX_AGE_MS, DRAFT_MAX_WAIT_MS, DRAFT_PAUSE_MS, draftKey, draftStanding, encodeDraft } from "./draft";
import { manualClock, memoryDraftStorage } from "./testing";

const KEY = "k";
const change = (body: string, pristineBody = "server") => ({ body, title: null, baseVersion: 3, pristineBody, pristineTitle: null });

const setup = (start = 1000) => {
  const clock = manualClock(start);
  const storage = memoryDraftStorage();
  return { clock, storage, keeper: createDraftKeeper({ clock, storage, key: KEY }) };
};

describe("createDraftKeeper, on ManualClock", () => {
  it("waits for typing to pause before writing", () => {
    const { clock, storage, keeper } = setup();
    keeper.edit(change("a"));
    keeper.flushIfDue();
    expect(storage.values.size).toBe(0);
    clock.advance(DRAFT_PAUSE_MS - 1);
    keeper.flushIfDue();
    expect(storage.values.size).toBe(0);
    clock.advance(1);
    keeper.flushIfDue();
    expect(keeper.load()?.body).toBe("a");
    expect(keeper.load()?.savedAt).toBe(1000 + DRAFT_PAUSE_MS);
  });

  it("writes during steady typing after the longest wait, so a crash loses little", () => {
    const { clock, storage, keeper } = setup();
    let elapsed = 0;
    while (elapsed < DRAFT_MAX_WAIT_MS) {
      keeper.edit(change(`text ${String(elapsed)}`));
      keeper.flushIfDue();
      expect(storage.values.size).toBe(0);
      clock.advance(DRAFT_PAUSE_MS - 1);
      elapsed += DRAFT_PAUSE_MS - 1;
    }
    keeper.edit(change("latest"));
    keeper.flushIfDue();
    expect(keeper.load()?.body).toBe("latest");
  });

  it("flushes at once when the page is going away", () => {
    const { storage, keeper } = setup();
    keeper.edit(change("a"));
    keeper.flush();
    expect(storage.values.size).toBe(1);
  });

  it("survives a reload: a new keeper on the same storage finds it", () => {
    const { clock, storage, keeper } = setup();
    keeper.edit({ body: "typed", title: "T", baseVersion: 3, pristineBody: "server", pristineTitle: "S" });
    keeper.flush();
    const reloaded = createDraftKeeper({ clock, storage, key: KEY });
    expect(reloaded.load()).toEqual({ body: "typed", title: "T", baseVersion: 3, savedAt: 1000 });
  });

  it("discards for good, including a write that was waiting", () => {
    const { clock, storage, keeper } = setup();
    keeper.edit(change("a"));
    keeper.flush();
    keeper.edit(change("b"));
    keeper.discard();
    clock.advance(DRAFT_MAX_WAIT_MS);
    keeper.flushIfDue();
    keeper.flush();
    expect(storage.values.size).toBe(0);
    expect(keeper.load()).toBeNull();
  });

  it("removes the draft when the text is back to what the server has", () => {
    const { storage, keeper } = setup();
    keeper.edit(change("a"));
    keeper.flush();
    expect(storage.values.size).toBe(1);
    keeper.edit(change("server"));
    keeper.flush();
    expect(storage.values.size).toBe(0);
  });

  it("counts a changed title as a draft, even with the body unchanged", () => {
    const { storage, keeper } = setup();
    keeper.edit({ body: "server", title: "New", baseVersion: null, pristineBody: "server", pristineTitle: "" });
    keeper.flush();
    expect(storage.values.size).toBe(1);
  });

  it("forgets a draft past its age, and keeps one exactly at it", () => {
    const { clock, storage, keeper } = setup();
    keeper.edit(change("a"));
    keeper.flush();
    clock.advance(DRAFT_MAX_AGE_MS);
    expect(keeper.load()?.body).toBe("a");
    clock.advance(1);
    expect(keeper.load()).toBeNull();
    expect(storage.values.size).toBe(0);
  });

  it("says when a write failed, and clears that when one works", () => {
    const { storage, keeper } = setup();
    storage.failing = true;
    keeper.edit(change("a"));
    keeper.flush();
    expect(keeper.writeFailed()).toBe(true);
    storage.failing = false;
    keeper.edit(change("b"));
    keeper.flush();
    expect(keeper.writeFailed()).toBe(false);
    expect(keeper.load()?.body).toBe("b");
  });

  it("does nothing with nothing to write", () => {
    const { storage, keeper } = setup();
    keeper.flushIfDue();
    keeper.flush();
    expect(storage.values.size).toBe(0);
    expect(keeper.load()).toBeNull();
  });
});

describe("decodeDraft", () => {
  it("round-trips, unicode and all", () => {
    const draft = { body: "å\n日本語 ", title: null, baseVersion: 0, savedAt: epochMs(5) };
    expect(decodeDraft(encodeDraft(draft))).toEqual(draft);
  });

  it("reads anything else in storage as no draft", () => {
    for (const bad of [null, "", "not json", "null", "3", "[]", "{}", '{"body":1,"title":null,"baseVersion":1,"savedAt":1}', '{"body":"a","title":1,"baseVersion":1,"savedAt":1}', '{"body":"a","title":null,"baseVersion":-1,"savedAt":1}', '{"body":"a","title":null,"baseVersion":1.5,"savedAt":1}', '{"body":"a","title":null,"baseVersion":1}', '{"body":"a","title":null,"baseVersion":"1","savedAt":1}']) {
      expect(decodeDraft(bad)).toBeNull();
    }
  });
});

describe("draftKey", () => {
  it("keeps people, vaults and pages apart", () => {
    const keys = [draftKey("p", vaultId("v"), "a"), draftKey("q", vaultId("v"), "a"), draftKey("p", vaultId("w"), "a"), draftKey("p", vaultId("v"), "b")];
    expect(new Set(keys).size).toBe(4);
  });
});

describe("draftStanding", () => {
  const draft = (baseVersion: number | null) => ({ body: "", title: null, baseVersion, savedAt: epochMs(0) });
  it("sees a page that has moved on", () => {
    expect(draftStanding(draft(3), 5)).toEqual({ kind: "behind", baseVersion: 3, pageVersion: 5 });
    expect(draftStanding(draft(5), 5)).toEqual({ kind: "current" });
    expect(draftStanding(draft(null), 5)).toEqual({ kind: "current" });
  });
});
