import { describe, expect, it } from "vitest";
import { actorText, jobActionText } from "./actors";

describe("actorText", () => {
  it("names a person and an agent", () => {
    expect(actorText("p1", { id: "p1", name: "Pat", kind: "human", disabled: false })).toEqual({ name: "Pat", kind: "Person", note: null });
    expect(actorText("a1", { id: "a1", name: "indexer", kind: "agent", disabled: false })).toEqual({ name: "indexer", kind: "Agent", note: null });
  });

  it("says a disabled principal is disabled", () => {
    expect(actorText("p1", { id: "p1", name: "Pat", kind: "human", disabled: true }).note).toBe("disabled");
  });

  it("says clearly that a principal that's gone is unknown, rather than failing", () => {
    const text = actorText("gone-1", { id: "gone-1", name: null, kind: "unknown", disabled: false });
    expect(text).toEqual({ name: "Unknown principal gone-1", kind: "Unknown", note: "no longer exists, or never was a principal" });
  });

  it("falls back to the id while the name isn't known", () => {
    expect(actorText("p1", null)).toEqual({ name: "p1", kind: "Unknown", note: null });
  });

  it("keeps a name holding markup as it is, for the screen to show as text", () => {
    expect(actorText("p1", { id: "p1", name: "<img src=x onerror=alert(1)>", kind: "human", disabled: false }).name).toBe("<img src=x onerror=alert(1)>");
  });
});

describe("jobActionText", () => {
  it("words each action", () => {
    expect(jobActionText({ action: "cancel", actor: "p", timeMs: 1 })).toBe("Cancelled by");
    expect(jobActionText({ action: "retry", actor: "p", timeMs: 1 })).toBe("Retried by");
    expect(jobActionText({ action: "pause", actor: "p", timeMs: 1 })).toBe("pause by");
  });
});
