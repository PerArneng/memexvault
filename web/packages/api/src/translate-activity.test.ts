import { describe, expect, it } from "vitest";
import { activityFrameFromWire, jobFromWire } from "./translate-activity";

describe("activityFrameFromWire", () => {
  it("reads a durable frame, and marks what it can't place", () => {
    const f = activityFrameFromWire("durable", JSON.stringify({ seq: 3, time_ms: 9, kind: "state.recorded", jobs: [{ id: "j", kind: "embed", state: "queued", status: "queued", attempts: 0, max_attempts: 3, next_run_at_ms: 1 }], counts: { job: 1 } }));
    expect(f).toMatchObject({ kind: "durable", seq: 3, jobs: [{ id: "j", sourceId: null }], unplaced: true });
  });
  it("doesn't mark a job that's about a document", () => {
    const f = activityFrameFromWire("durable", JSON.stringify({ seq: 3, time_ms: 9, kind: "state.recorded", jobs: [{ id: "j", kind: "rendition", state: "queued", status: "queued", attempts: 0, max_attempts: 3, next_run_at_ms: 1, source_id: "s" }], counts: { job: 1 } }));
    expect(f).toMatchObject({ unplaced: false });
  });
  it("treats a frame cut short, or new rows that decide a stage, as unplaced", () => {
    expect(activityFrameFromWire("durable", JSON.stringify({ seq: 1, time_ms: 1, kind: "x", omitted: 2 }))).toMatchObject({ unplaced: true });
    expect(activityFrameFromWire("durable", JSON.stringify({ seq: 1, time_ms: 1, kind: "changeset.recorded", counts: { changeset: 1 } }))).toMatchObject({ unplaced: true });
  });
  it("reads the others", () => {
    expect(activityFrameFromWire("progress", JSON.stringify({ job_id: "j", source_id: "s", stage: "ocr", done: 12, total: 400, time_ms: 5 }))).toEqual({ kind: "progress", jobId: "j", sourceId: "s", stage: "ocr", done: 12, total: 400, timeMs: 5 });
    expect(activityFrameFromWire("resync", '{"head":4,"reason":"erased"}')).toEqual({ kind: "resync", head: 4, reason: "erased" });
    expect(activityFrameFromWire("hello", '{"head":-1,"resumed":false}')).toEqual({ kind: "hello", head: -1, resumed: false });
    expect(activityFrameFromWire("heartbeat", '{"head":4}')).toEqual({ kind: "heartbeat", head: 4 });
    expect(activityFrameFromWire("end", '{"reason":"scope_lost"}')).toEqual({ kind: "end", reason: "scope_lost" });
  });
  it("calls anything else unknown rather than failing", () => {
    expect(activityFrameFromWire("from-the-future", "{}")).toEqual({ kind: "unknown", name: "from-the-future" });
    expect(activityFrameFromWire("durable", "not json")).toEqual({ kind: "unknown", name: "durable" });
    expect(activityFrameFromWire("durable", "[1]")).toEqual({ kind: "unknown", name: "durable" });
    expect(activityFrameFromWire("durable", "{}")).toEqual({ kind: "unknown", name: "durable" });
  });
});

describe("jobFromWire", () => {
  it("takes the source from the input, and an attempt whatever its shape", () => {
    const job = jobFromWire({ id: "j", kind: "rendition", state: "dead", input: { source_id: "s" }, max_attempts: 3, next_run_at_ms: 1, created_at_ms: 2, attempts: [{ started_at_ms: 1, finished_at_ms: 2, error: "boom" }, {}] as never[] });
    expect(job.sourceId).toBe("s");
    expect(job.attempts).toEqual([{ startedAtMs: 1, finishedAtMs: 2, error: "boom" }, { startedAtMs: 0, finishedAtMs: null, error: null }]);
  });
});
