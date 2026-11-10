import { describe, expect, it } from "vitest";
import { eventOfFrame } from "./queue-feed";

describe("eventOfFrame", () => {
  it("a change to a ChangeSet moves the queue", () => {
    expect(eventOfFrame(JSON.stringify({ seq: 1, kind: "changeset.recorded", changesets: [{ id: "c" }] }))).toEqual({ kind: "changed" });
  });
  it("a frame too full to list them might", () => {
    expect(eventOfFrame(JSON.stringify({ seq: 1, kind: "state.recorded", omitted: 3 }))).toEqual({ kind: "changed" });
  });
  it("other work doesn't, and frames leave out empty lists", () => {
    expect(eventOfFrame(JSON.stringify({ seq: 1, kind: "segment.recorded" }))).toBeNull();
  });
  it("a frame it can't read counts as a change, as reading again costs little", () => {
    expect(eventOfFrame("not json")).toEqual({ kind: "changed" });
  });
});
