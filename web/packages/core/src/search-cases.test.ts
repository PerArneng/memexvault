import { vaultId } from "@memex/types";
import { describe, expect, it } from "vitest";
import { createEngine } from "./engine";
import type { Engine } from "./engine";
import { EMPTY_SEARCH, searchRequestOf } from "./search";
import { DEFAULT_SETTINGS } from "./settings";
import { countingIds, fakeApi, httpFailure, manualClock, memoryDraftStorage, memoryPreferences } from "./testing";
import type { FakeApi } from "./testing";

const V = vaultId("v1");
const asked = searchRequestOf({ ...EMPTY_SEARCH, q: "a" });
if (asked === null) throw new Error("a query with text is a request");
const request = asked;

const setup = (): { readonly api: FakeApi; readonly engine: Engine } => {
  const api = fakeApi({});
  return { api, engine: createEngine({ clock: manualClock(1000), settings: DEFAULT_SETTINGS, api, preferences: memoryPreferences(), drafts: memoryDraftStorage(), ids: countingIds() }) };
};

describe("search", () => {
  it("answers through the API", async () => {
    const { api, engine } = setup();
    const answer = await engine.search(V, request, null);
    expect(answer.ok && answer.value.items).toEqual([]);
    expect(api.calls).toEqual(["search"]);
  });

  it("counts no answer against the connection, unless the search was typed past", async () => {
    const { api, engine } = setup();
    api.down = true;
    await engine.search(V, request, { aborted: true });
    expect(engine.connectivity()).toEqual({ status: "online", failures: 0 });
    await engine.search(V, request, { aborted: false });
    expect(engine.connectivity()).toEqual({ status: "online", failures: 1 });
  });

  it("gives a failure back as data", async () => {
    const { api, engine } = setup();
    api.failNext(httpFailure(400, "invalid", "that cursor isn't one this API gave out"));
    const answer = await engine.search(V, request, null);
    expect(!answer.ok && answer.failure.kind).toBe("http");
  });
});
