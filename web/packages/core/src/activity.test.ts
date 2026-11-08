import type { ActivityFrame, DocumentActivity, DurableFrame, JobFact } from "@memex/types";
import { describe, expect, it } from "vitest";
import { activityReducer, boardRows, framesPerSecond, INITIAL_ACTIVITY, MAX_BOARD_DOCUMENTS, pipelineSteps, progressText, stageCounts, statusExplanation, throughputPerMinute } from "./activity";
import type { ActivityEvent, ActivityState } from "./activity";
import { documentActivity } from "./ingest-testing";
import { manualClock } from "./testing";

const clock = manualClock(1000);
const durable = (seq: number, over: Partial<DurableFrame> = {}): DurableFrame => ({ kind: "durable", seq, timeMs: 1, eventKind: "state.recorded", sources: [], jobs: [], renditions: [], extractions: [], unplaced: false, ...over });
const job = (id: string, over: Partial<JobFact> = {}): JobFact => ({ id, kind: "rendition", state: "running", stage: "parsing", status: "active", attempts: 1, nextRunAtMs: 0, waitingSinceMs: null, sourceId: "s1", ...over });
const frame = (f: ActivityFrame): ActivityEvent => ({ type: "frame", frame: f, at: clock.now() });
const snapshot = (head: number, documents: readonly DocumentActivity[], replay: readonly DurableFrame[] = []): ActivityEvent => ({ type: "snapshot", head, documents, truncated: false, replay, at: clock.now() });
const start = (head = 10, documents: readonly DocumentActivity[] = [documentActivity("s1")]): ActivityState => activityReducer(INITIAL_ACTIVITY, [snapshot(head, documents)]);
const doc = (state: ActivityState, id = "s1") => {
  const found = state.documents.get(id);
  if (found === undefined) throw new Error(`no ${id}`);
  return found;
};

describe("activityReducer", () => {
  it("merges a snapshot and then events that follow it", () => {
    let s = start();
    expect(s.seq).toBe(10);
    s = activityReducer(s, [frame(durable(11, { jobs: [job("j1")] }))]);
    expect(doc(s)).toMatchObject({ stage: "parsing", status: "active", jobId: "j1", attempts: 1 });
    expect(s.seq).toBe(11);
  });

  it("creates a document from its source fact", () => {
    const s = activityReducer(start(10, []), [frame(durable(11, { sources: [{ id: "new", logicalId: "new", versionNo: 1 }] }))]);
    expect(doc(s, "new")).toMatchObject({ stage: "stored", status: "queued" });
  });

  it("ignores a duplicate frame", () => {
    const one = activityReducer(start(), [frame(durable(11, { jobs: [job("j1")] }))]);
    const two = activityReducer(one, [frame(durable(11, { jobs: [job("j1", { state: "dead", status: "failed" })] }))]);
    expect(doc(two).status).toBe("active");
    expect(two.ignoredFrames).toBe(1);
  });

  it("ignores an out-of-order frame and asks for a snapshot about the gap it shows", () => {
    const ahead = activityReducer(start(), [frame(durable(13, { jobs: [job("j1", { state: "running", status: "active" })] }))]);
    expect(ahead.needsSnapshot).toBe(true);
    const late = activityReducer(ahead, [frame(durable(12, { jobs: [job("j1", { state: "queued", status: "queued", attempts: 0 })] }))]);
    expect(doc(late).status).toBe("active");
    expect(late.seq).toBe(13);
    expect(late.ignoredFrames).toBe(1);
  });

  it("doesn't count a late frame as a gap once it's beyond the position", () => {
    const s = activityReducer(start(), [frame(durable(11)), frame(durable(12))]);
    expect(s.needsSnapshot).toBe(false);
  });

  it("a resync moves the position, even back, and asks for a snapshot", () => {
    const moved = activityReducer(start(10), [frame({ kind: "resync", head: 4, reason: "erased" })]);
    expect(moved.seq).toBe(4);
    expect(moved.needsSnapshot).toBe(true);
    const next = activityReducer(moved, [frame(durable(5))]);
    expect(next.seq).toBe(5);
    expect(next.ignoredFrames).toBe(0);
  });

  it("a snapshot clears what it was asked for, drops what's gone and replays only what's after it", () => {
    const needing = activityReducer(start(10, [documentActivity("s1"), documentActivity("gone")]), [frame({ kind: "resync", head: 12, reason: "overflow" })]);
    const s = activityReducer(needing, [snapshot(13, [documentActivity("s1", { stage: "embedding", status: "active" })], [durable(12, { jobs: [job("old", { state: "dead", status: "failed" })] }), durable(14, { jobs: [job("j2", { kind: "extract", stage: "extraction" })] })])]);
    expect(s.documents.has("gone")).toBe(false);
    expect(s.needsSnapshot).toBe(false);
    expect(s.seq).toBe(14);
    expect(doc(s).stage).toBe("embedding");
    expect(s.dirty).toBe(true);
  });

  it("an unknown frame or event kind changes nothing but moves the position", () => {
    const s = activityReducer(start(), [frame({ kind: "unknown", name: "from-the-future" }), frame(durable(11, { eventKind: "other" }))]);
    expect(s.unknownFrames).toBe(1);
    expect(s.seq).toBe(11);
    expect(doc(s).stage).toBe("stored");
  });

  it("an unknown stage from a newer server still shows, and is counted under its own name", () => {
    const s = start(1, [documentActivity("s1", { stage: "quantum", status: "active" })]);
    expect(stageCounts(s.documents.values()).find((c) => c.stage === "quantum")?.count).toBe(1);
    expect(pipelineSteps(doc(s)).at(-1)).toMatchObject({ stage: "quantum", state: "current" });
  });

  it("frames that say nothing placeable mark it for a snapshot soon", () => {
    expect(activityReducer(start(), [frame(durable(11, { unplaced: true }))]).dirty).toBe(true);
    expect(activityReducer(start(), [frame(durable(11, { jobs: [job("e", { kind: "embed", stage: "embedding", sourceId: null })] }))]).dirty).toBe(true);
  });

  it("a batch of events gives one new state and an empty batch gives the same one", () => {
    const s = start();
    expect(activityReducer(s, [])).toBe(s);
  });
});

describe("the stages of one document", () => {
  it("walks the stages in order, and times each from the clock", () => {
    let s = start(10, [documentActivity("s1", { recordedAtMs: 500 })]);
    const steps: [ActivityFrame, string][] = [
      [durable(11, { jobs: [job("j1")] }), "parsing"],
      [{ kind: "progress", jobId: "j1", sourceId: "s1", stage: "ocr", done: 12, total: 400, timeMs: 2 }, "ocr"],
      [{ kind: "progress", jobId: "j1", sourceId: "s1", stage: "segmenting", done: 0, total: null, timeMs: 3 }, "segmenting"],
      [durable(12, { jobs: [job("j1", { state: "done", status: "done" })], renditions: [{ id: "r1", sourceId: "s1", pageCount: 400, segmentCount: 9, superseded: false }] }), "indexing"],
    ];
    for (const [f, stage] of steps) {
      clock.advance(100);
      s = activityReducer(s, [frame(f)]);
      expect(doc(s).stage).toBe(stage);
      expect(doc(s).stageSinceMs).toBe(clock.now());
    }
    expect(doc(s).history.map((h) => h.stage)).toEqual(["stored", "parsing", "ocr", "segmenting", "indexing"]);
    expect(doc(s)).toMatchObject({ renditionId: "r1", pageCount: 400, segmentCount: 9, jobId: null });
    expect(pipelineSteps(doc(s)).map((p) => [p.stage, p.state])).toEqual([["stored", "seen"], ["parsing", "seen"], ["ocr", "seen"], ["segmenting", "seen"], ["indexing", "current"], ["embedding", "todo"], ["extraction", "todo"], ["review", "todo"], ["done", "todo"]]);
  });

  it("a recording goes through transcription rather than parsing", () => {
    const s = activityReducer(start(), [frame(durable(11, { jobs: [job("t1", { kind: "transcription", stage: "transcription" })] }))]);
    expect(pipelineSteps(doc(s)).map((p) => p.stage)).toEqual(["stored", "transcription", "indexing", "embedding", "extraction", "review", "done"]);
  });

  it("knows the way through from durable facts after a reload, with nothing seen live", () => {
    const s = start(1, [documentActivity("s1", { stage: "done", status: "done" })]);
    expect(pipelineSteps(doc(s), { recording: true, ocr: false }).map((p) => p.stage)).toEqual(["stored", "transcription", "indexing", "embedding", "extraction", "review", "done"]);
    expect(pipelineSteps(doc(s), { recording: false, ocr: true }).map((p) => p.stage)).toEqual(["stored", "parsing", "ocr", "segmenting", "indexing", "embedding", "extraction", "review", "done"]);
  });

  it("marks a stage that went by unseen rather than leaving it out", () => {
    const s = activityReducer(start(), [snapshot(11, [documentActivity("s1", { stage: "embedding", status: "active" })])]);
    expect(pipelineSteps(doc(s)).slice(0, 5).map((p) => p.state)).toEqual(["seen", "passed", "passed", "passed", "current"]);
  });

  it("progress can come before the frame that says the job runs, and that frame doesn't undo it", () => {
    let s = activityReducer(start(), [frame({ kind: "progress", jobId: "j1", sourceId: "s1", stage: "ocr", done: 1, total: 2, timeMs: 5 })]);
    expect(doc(s)).toMatchObject({ stage: "ocr", status: "active" });
    s = activityReducer(s, [frame(durable(11, { jobs: [job("j1")] }))]);
    expect(doc(s).stage).toBe("ocr");
  });

  it("older progress, progress from a finished job and progress for no one are all ignored", () => {
    let s = activityReducer(start(), [frame({ kind: "progress", jobId: "j1", sourceId: "s1", stage: "ocr", done: 5, total: 9, timeMs: 50 })]);
    s = activityReducer(s, [frame({ kind: "progress", jobId: "j1", sourceId: "s1", stage: "ocr", done: 4, total: 9, timeMs: 40 })]);
    expect(doc(s).progress?.done).toBe(5);
    s = activityReducer(s, [frame(durable(11, { jobs: [job("j1", { state: "done", status: "done" })] }))]);
    const after = activityReducer(s, [frame({ kind: "progress", jobId: "j1", sourceId: "s1", stage: "ocr", done: 9, total: 9, timeMs: 60 }), frame({ kind: "progress", jobId: "j9", sourceId: "nobody", stage: "ocr", done: 1, total: 2, timeMs: 61 })]);
    expect(doc(after).stage).toBe("indexing");
    expect(doc(after).progress).toBeNull();
  });

  it("dead and cancelled read as failed, a retry brings it back, and a later stage can't hide the failure", () => {
    let s = activityReducer(start(), [frame(durable(11, { jobs: [job("j1", { state: "dead", status: "failed", attempts: 3 })] }))]);
    expect(doc(s)).toMatchObject({ stage: "parsing", status: "failed", attempts: 3, jobId: "j1" });
    s = activityReducer(s, [frame(durable(12, { jobs: [job("x1", { kind: "extract", stage: "extraction" })] }))]);
    expect(doc(s).stage).toBe("parsing");
    s = activityReducer(s, [frame(durable(13, { jobs: [job("j1", { state: "queued", status: "retrying", attempts: 3 })] }))]);
    expect(doc(s)).toMatchObject({ stage: "parsing", status: "retrying" });
  });

  it("extraction beside embedding leaves the stage to the stage model, as progress does, and follows it once there", () => {
    const embedding = start(10, [documentActivity("s1", { stage: "embedding", status: "active" })]);
    let s = activityReducer(embedding, [frame(durable(11, { jobs: [job("x1", { kind: "extract", stage: "extraction" })] })), frame({ kind: "progress", jobId: "x1", sourceId: "s1", stage: "extraction", done: 1, total: 3, timeMs: 5 })]);
    expect(doc(s)).toMatchObject({ stage: "embedding", progress: { done: 1 } });
    expect(s.dirty).toBe(true);
    s = activityReducer(s, [snapshot(11, [documentActivity("s1", { stage: "extraction", status: "active", jobId: "x1" })])]);
    s = activityReducer(s, [frame(durable(12, { jobs: [job("x1", { kind: "extract", stage: "extraction", state: "queued", status: "waiting", waitingSinceMs: 7 })] }))]);
    expect(doc(s)).toMatchObject({ stage: "extraction", status: "waiting", waitingSinceMs: 7 });
  });

  it("an older job can't overwrite what a newer one says", () => {
    let s = activityReducer(start(), [frame(durable(11, { jobs: [job("j2")] }))]);
    s = activityReducer(s, [frame(durable(12, { jobs: [job("j1", { state: "dead", status: "failed" })] }))]);
    expect(doc(s)).toMatchObject({ status: "active", jobId: "j2" });
  });

  it("says in plain words that waiting for the LLM isn't a failure", () => {
    const text = statusExplanation(documentActivity("s1", { stage: "extraction", status: "waiting", waitingSinceMs: 1000 }), 91_000);
    expect(text).toContain("Waiting for the LLM");
    expect(text).toContain("Nothing has failed");
    expect(text).toContain("1 min 30 s");
  });

  it("says when a retry comes", () => {
    expect(statusExplanation(documentActivity("s1", { status: "retrying", nextRunAtMs: 5000 }), 1000)).toContain("in 4 s");
    expect(statusExplanation(documentActivity("s1", { status: "active" }), 1000)).toBeNull();
  });

  it("writes OCR page 12 of 400", () => {
    expect(progressText({ stage: "ocr", done: 12, total: 400, timeMs: 0 })).toBe("OCR page 12 of 400");
    expect(progressText({ stage: "segmenting", done: 0, total: null, timeMs: 0 })).toBeNull();
    expect(progressText(null)).toBeNull();
  });
});

describe("what the board shows", () => {
  it("counts per stage with every known stage present, and the failed in each", () => {
    const counts = stageCounts([documentActivity("a", { stage: "ocr", status: "failed" }), documentActivity("b", { stage: "ocr", status: "active" })]);
    expect(counts.find((c) => c.stage === "ocr")).toEqual({ stage: "ocr", count: 2, failed: 1 });
    expect(counts.find((c) => c.stage === "done")?.count).toBe(0);
  });

  it("puts what needs a person first and what's finished last, and filters", () => {
    const s = start(1, [documentActivity("done1", { stage: "done", status: "done" }), documentActivity("run1", { stage: "ocr", status: "active" }), documentActivity("bad1", { stage: "parsing", status: "failed" }), documentActivity("wait1", { stage: "extraction", status: "waiting" })]);
    const none = { stage: "", status: "", text: "" };
    const names = (id: string) => (id === "run1" ? "Annual report.PDF" : undefined);
    expect(boardRows(s.documents.values(), none, names).map((d) => d.sourceId)).toEqual(["bad1", "wait1", "run1", "done1"]);
    expect(boardRows(s.documents.values(), { ...none, stage: "ocr" }, names).map((d) => d.sourceId)).toEqual(["run1"]);
    expect(boardRows(s.documents.values(), { ...none, status: "failed" }, names).map((d) => d.sourceId)).toEqual(["bad1"]);
    expect(boardRows(s.documents.values(), { ...none, text: "annual" }, names).map((d) => d.sourceId)).toEqual(["run1"]);
  });

  it("measures throughput from finished documents, over at least a minute, and forgets old ones", () => {
    const local = manualClock(1_000_000);
    let s = activityReducer(INITIAL_ACTIVITY, [{ type: "snapshot", head: 1, truncated: false, replay: [], at: local.now(), documents: [documentActivity("a", { stage: "review", status: "active" }), documentActivity("b", { stage: "review", status: "active" })] }]);
    s = activityReducer(s, [{ type: "snapshot", head: 2, truncated: false, replay: [], at: local.now(), documents: [documentActivity("a", { stage: "done", status: "done" }), documentActivity("b", { stage: "done", status: "done" })] }]);
    expect(throughputPerMinute(s, local.now())).toBe(2);
    local.advance(400_000);
    expect(throughputPerMinute(s, local.now())).toBe(0);
  });

  it("measures events per second over ten seconds, and keeps only that many", () => {
    const local = manualClock(1000);
    let s = start();
    for (let i = 0; i < 50; i += 1) {
      local.advance(100);
      s = activityReducer(s, [{ type: "frame", frame: { kind: "heartbeat", head: 1 }, at: local.now() }, { type: "frame", frame: { kind: "progress", jobId: "j", sourceId: "s1", stage: "ocr", done: i, total: 50, timeMs: i }, at: local.now() }]);
    }
    expect(s.frameTimes.length).toBeLessThanOrEqual(100);
    expect(framesPerSecond(s, local.now())).toBe(5);
  });

  it("holds no more documents than the cap, and says so", () => {
    const many = Array.from({ length: MAX_BOARD_DOCUMENTS }, (_, i) => documentActivity(`d${String(i)}`));
    const s = activityReducer(start(1, many), [frame(durable(2, { sources: [{ id: "one-more", logicalId: null, versionNo: 1 }] }))]);
    expect(s.documents.size).toBe(MAX_BOARD_DOCUMENTS);
    expect(s.truncated).toBe(true);
  });
});

describe("the connection", () => {
  it("goes from connecting to live to reconnecting, and a return asks for a snapshot", () => {
    let s = start();
    expect(s.connection.status).toBe("connecting");
    s = activityReducer(s, [frame({ kind: "hello", head: 10, resumed: true })]);
    expect(s.connection.status).toBe("live");
    expect(s.needsSnapshot).toBe(false);
    s = activityReducer(s, [{ type: "lost", at: 5 }, { type: "lost", at: 9 }]);
    expect(s.connection).toEqual({ status: "reconnecting", sinceMs: 5 });
    s = activityReducer(s, [frame({ kind: "hello", head: 10, resumed: true })]);
    expect(s.connection.status).toBe("live");
    expect(s.needsSnapshot).toBe(true);
  });

  it("stays ended once ended", () => {
    const s = activityReducer(start(), [{ type: "ended", reason: "scope_lost" }, { type: "lost", at: 1 }]);
    expect(s.connection).toEqual({ status: "ended", reason: "scope_lost" });
  });
});
