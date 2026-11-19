import { INLINE_CLOSE, INLINE_OPEN, vaultId } from "@memex/types";
import type { AgentEvent, ChannelEvent, ChatStoredMessage, ChatToolAnswer, EvidenceItem, SearchCitation } from "@memex/types";
import { epochMs } from "@memex/types";
import { describe, expect, it } from "vitest";
import { attachableFrom, citationIndex, EMPTY_TRANSCRIPT, messageWith, prepareReply, replyParts, stepSummary, transcriptOf, transcriptReducer, unresolvedCount } from "./chat";
import type { TranscriptAction, TranscriptEntry, TranscriptState } from "./chat";

const v = vaultId("v1");
const sent: TranscriptAction = { type: "sent", runId: "r1", messageId: "u1", text: "How long is the bridge?" };
const ev = (event: AgentEvent): TranscriptAction => ({ type: "channel", runId: "r1", event: { kind: "event", event } });
const ch = (event: ChannelEvent, runId = "r1"): TranscriptAction => ({ type: "channel", runId, event });
const play = (...actions: TranscriptAction[]): TranscriptState => actions.reduce(transcriptReducer, EMPTY_TRANSCRIPT);
const kinds = (s: TranscriptState) => s.entries.map((e) => e.kind);
const replies = (s: TranscriptState) => s.entries.filter((e): e is Extract<TranscriptEntry, { kind: "reply" }> => e.kind === "reply");
const steps = (s: TranscriptState) => s.entries.filter((e): e is Extract<TranscriptEntry, { kind: "step" }> => e.kind === "step");
const notices = (s: TranscriptState) => s.entries.filter((e): e is Extract<TranscriptEntry, { kind: "notice" }> => e.kind === "notice");

const started = ev({ type: "run_started", runId: "r1", threadId: "t1" });
const textStart = ev({ type: "text_start", messageId: "m1" });
const delta = (d: string, messageId = "m1") => ev({ type: "text_delta", messageId, delta: d });
const textEnd = ev({ type: "text_end", messageId: "m1" });
const finished = (stop: string | null = null) => ev({ type: "run_finished", runId: "r1", stop });
const closed = ch({ kind: "closed" });

const citation = (over: Partial<SearchCitation> = {}): SearchCitation => ({ anchor: "located", sourceId: "s1", sourceName: "a.pdf", sourceSha256: "x", trustTier: "trusted", segmentId: "seg1", evidenceId: null, renditionId: "r1", pageFrom: 2, pageTo: 2, rects: [{ page: 4, x0: 1, y0: 2, x1: 3, y1: 4 }], quote: null, tStartMs: null, tEndMs: null, speaker: null, ...over });
const item = (over: Partial<EvidenceItem> = {}): EvidenceItem => ({ id: "seg1", kind: "segment", text: "t", heading: null, language: null, epistemic: null, trustTiers: [], validFromMs: null, validToMs: null, score: null, pageId: null, citations: [citation()], conflicts: [], ...over });
const searchAnswer = (items: EvidenceItem[]): ChatToolAnswer => ({ kind: "search", items, notices: [] });

describe("the transcript reducer, one event kind at a time", () => {
  it("adds the person's message and starts a run on send, and refuses a second send while one runs", () => {
    const s = play(sent);
    expect(s.status).toBe("running");
    expect(s.entries).toEqual([{ kind: "user", id: "u1", text: "How long is the bridge?" }]);
    const again = transcriptReducer(s, { ...sent, runId: "r2" });
    expect(again.entries).toHaveLength(1);
    expect(again.runId).toBe("r1");
  });

  it("streams text into one reply and ends it", () => {
    const s = play(sent, started, textStart, delta("The bridge "), delta("is 412 m [cite:seg1]."), textEnd, finished(), closed);
    expect(replies(s)).toEqual([{ kind: "reply", id: "m1", text: "The bridge is 412 m [cite:seg1].", end: "done" }]);
    expect(s.status).toBe("idle");
    expect(notices(s)).toEqual([]);
    expect(s.ignored).toBe(0);
  });

  it("builds a tool step from start, args, end and result", () => {
    const s = play(
      sent,
      started,
      ev({ type: "tool_start", toolCallId: "c1", name: "search" }),
      ev({ type: "tool_args", toolCallId: "c1", delta: '{"q":' }),
      ev({ type: "tool_args", toolCallId: "c1", delta: '"bridge"}' }),
      ev({ type: "tool_end", toolCallId: "c1" }),
    );
    expect(steps(s)).toEqual([{ kind: "step", id: "c1", name: "search", args: '{"q":"bridge"}', state: "running", answer: null }]);
    const done = transcriptReducer(s, ev({ type: "tool_result", toolCallId: "c1", answer: searchAnswer([item()]) }));
    expect(steps(done)[0]).toMatchObject({ state: "done", answer: { kind: "search" } });
    expect(stepSummary(steps(done)[0] as Extract<TranscriptEntry, { kind: "step" }>)).toBe("Searched for “bridge” · 1 found");
  });

  it("marks a tool's error as a failed step, and the run carries on", () => {
    const s = play(sent, started, ev({ type: "tool_start", toolCallId: "c1", name: "fetch_url" }), ev({ type: "tool_result", toolCallId: "c1", answer: { kind: "error", code: "invalid", message: "there's no tool called 'fetch_url'" } }), textStart, delta("No."), textEnd, finished(), closed);
    expect(steps(s)[0]?.state).toBe("failed");
    expect(replies(s)[0]?.end).toBe("done");
    expect(s.status).toBe("idle");
  });

  it("ends a run whose result says why it stopped, and says so", () => {
    for (const [stop, end, tone] of [["max_tokens", "cut", "info"], ["thread_budget", "budget", "info"], ["day_budget", "budget", "info"], ["cancelled", "stopped", "stopped"], ["something_new", "cut", "info"]] as const) {
      const s = play(sent, started, textStart, delta("The bri"), finished(stop), closed);
      expect(replies(s)[0]?.end, stop).toBe(end);
      expect(notices(s)[0]?.tone, stop).toBe(tone);
      expect(s.status).toBe("idle");
    }
  });

  it("keeps a reply that RUN_ERROR cut off, marked as failed, with the server's fixed message and code", () => {
    const s = play(sent, started, textStart, delta("The bridge is"), ev({ type: "run_error", message: "the model provider failed", code: "provider_failed" }), closed);
    expect(replies(s)).toEqual([{ kind: "reply", id: "m1", text: "The bridge is", end: "failed" }]);
    expect(notices(s)).toEqual([{ kind: "notice", id: expect.any(String) as string, tone: "error", text: "the model provider failed (provider_failed)" }]);
    expect(s.status).toBe("idle");
  });

  it("shows the client's own abort as stopped, not failed, and keeps the partial reply", () => {
    const s = play(sent, started, textStart, delta("The bri"), { type: "stop_requested" }, ev({ type: "run_error", message: "Request aborted", code: "abort" }), closed);
    expect(replies(s)[0]).toMatchObject({ text: "The bri", end: "stopped" });
    expect(notices(s).map((n) => [n.tone, n.text])).toEqual([["stopped", "Stopped."]]);
    // An abort with no stop asked for, such as the page going away, is still a stop.
    expect(replies(play(sent, textStart, delta("x"), ev({ type: "run_error", message: "aborted", code: "abort" })))[0]?.end).toBe("stopped");
  });

  it("marks what arrived when the connection drops, and when a stream just ends with no end event", () => {
    const dropped = play(sent, started, textStart, delta("The bri"), ch({ kind: "dropped", message: "network error" }), closed);
    expect(replies(dropped)[0]?.end).toBe("dropped");
    expect(notices(dropped)[0]?.tone).toBe("error");
    expect(dropped.status).toBe("idle");
    const ended = play(sent, started, textStart, delta("The bri"), closed);
    expect(replies(ended)[0]?.end).toBe("dropped");
    expect(notices(ended)[0]?.text).toMatch(/without saying it had finished/);
    // A step left running ends too.
    expect(steps(play(sent, ev({ type: "tool_start", toolCallId: "c", name: "search" }), closed))[0]?.state).toBe("failed");
  });

  it("says why the server refused a run before it streamed", () => {
    const off = play(sent, ch({ kind: "refused", status: 403, code: "chat_disabled", message: "chat is off" }), closed);
    expect(notices(off)[0]?.text).toBe("Chat is off for this vault. An admin can turn it on.");
    expect(off.status).toBe("idle");
    expect(notices(play(sent, ch({ kind: "refused", status: 409, code: "conflict", message: "" })))[0]?.text).toMatch(/still being written/);
    expect(notices(play(sent, ch({ kind: "refused", status: 429, code: "busy", message: "" })))[0]?.text).toMatch(/as many chats/);
    expect(notices(play(sent, ch({ kind: "refused", status: 400, code: "invalid", message: "too long" })))[0]?.text).toBe("The server refused this message: too long");
  });

  it("takes events out of order without losing text, and ignores repeats", () => {
    // Content before its start, a start repeated, an end for a message that never was, args and a result before the tool's start.
    const s = play(
      sent,
      delta("The ", "m1"),
      textStart,
      delta("bridge."),
      ev({ type: "text_end", messageId: "nope" }),
      ev({ type: "tool_args", toolCallId: "c9", delta: '{"q":"x"}' }),
      ev({ type: "tool_result", toolCallId: "c8", answer: { kind: "other" } }),
      ev({ type: "tool_start", toolCallId: "c9", name: "search" }),
      textEnd,
      delta(" late"),
      finished(),
    );
    expect(replies(s)).toEqual([{ kind: "reply", id: "m1", text: "The bridge.", end: "done" }]);
    expect(steps(s).map((x) => [x.id, x.state])).toEqual([["c9", "done"], ["c8", "done"]]);
    expect(s.ignored).toBe(4);
    expect(s.status).toBe("idle");
  });

  it("ignores events about another run, and anything after the run ended", () => {
    const s = play(sent, textStart, delta("one"), finished(), ch({ kind: "event", event: { type: "text_delta", messageId: "m1", delta: "two" } }), ch({ kind: "event", event: { type: "text_start", messageId: "m9" } }, "r0"));
    expect(replies(s)).toEqual([{ kind: "reply", id: "m1", text: "one", end: "done" }]);
    expect(s.ignored).toBe(2);
  });

  it("ignores unknown events and changes nothing", () => {
    const before = play(sent, textStart, delta("a"));
    const after = transcriptReducer(before, ev({ type: "unknown", name: "STATE_DELTA" }));
    expect(after.entries).toBe(before.entries);
    expect(after.ignored).toBe(before.ignored + 1);
  });

  it("stops a run only while it's running", () => {
    expect(transcriptReducer(EMPTY_TRANSCRIPT, { type: "stop_requested" })).toBe(EMPTY_TRANSCRIPT);
    expect(play(sent, { type: "stop_requested" }).status).toBe("stopping");
    // A drop while stopping is the stop.
    expect(replies(play(sent, textStart, delta("x"), { type: "stop_requested" }, ch({ kind: "dropped", message: "" })))[0]?.end).toBe("stopped");
  });
});

describe("a thread as the server kept it", () => {
  const m = (seq: number, over: Partial<ChatStoredMessage>): ChatStoredMessage => ({ seq, id: `m${String(seq)}`, runId: "r", role: "assistant", text: "", toolCall: null, answer: null, stop: null, at: epochMs(seq), ...over });
  it("pairs each tool call with its answer, and keeps why a reply ended", () => {
    const entries = transcriptOf([
      m(5, { text: "The bri", stop: "cancelled" }),
      m(1, { role: "user", text: "q" }),
      m(2, { toolCall: { id: "c1", name: "search", args: '{"q":"b"}' } }),
      m(3, { role: "tool", text: "{}", toolCall: { id: "c1", name: "search", args: '{"q":"b"}' }, answer: searchAnswer([item()]) }),
      m(4, { text: "It is 412 m [cite:seg1]." }),
      m(6, { text: "x", stop: "max_tokens" }),
    ]);
    expect(entries.map((e) => e.kind)).toEqual(["user", "step", "reply", "reply", "reply"]);
    expect(entries[1]).toMatchObject({ id: "c1", state: "done", answer: { kind: "search" } });
    expect(entries.slice(2).map((e) => (e.kind === "reply" ? e.end : null))).toEqual(["done", "stopped", "cut"]);
    expect(kinds(play({ type: "loaded", messages: [m(1, { role: "user", text: "q" })] }))).toEqual(["user"]);
  });
});

describe("citations", () => {
  it("open the region a search returned: a segment's page and passage, and a claim's evidence", () => {
    const claim = item({ id: "c1", kind: "claim", citations: [citation({ evidenceId: "e1", segmentId: "seg2" })] });
    const recording = item({ id: "seg3", citations: [citation({ sourceId: "s2", segmentId: "seg3", rects: [], pageFrom: null, tStartMs: 61_000, tEndMs: 64_500 })] });
    const index = citationIndex(v, [{ kind: "step", id: "c", name: "search", args: "", state: "done", answer: searchAnswer([item(), claim, recording]) }]);
    expect(index.get("seg1")).toBe("/vaults/v1/documents/s1?page=5&segment=seg1");
    expect(index.get("c1")).toBe("/vaults/v1/documents/s1?page=5&claim=c1&evidence=e1");
    expect(index.get("seg2")).toBe("/vaults/v1/documents/s1?page=5&segment=seg2");
    expect(index.get("seg3")).toBe("/vaults/v1/documents/s2?t=61000&end=64500&segment=seg3");
  });

  it("open what a read returned: a segment, a claim's lineage, a page and an entity", () => {
    const entries: TranscriptEntry[] = [
      { kind: "step", id: "a", name: "read", args: "", state: "done", answer: { kind: "segment", segment: { id: "g9", sourceId: "s9", renditionId: null, text: "t", start: 0, end: 1, heading: null, language: null, pageFrom: 1, pageTo: 1, speaker: null, tStartMs: null, tEndMs: null } } },
      { kind: "step", id: "b", name: "read", args: "", state: "done", answer: { kind: "object", objectKind: "page", id: "p1", title: "P" } },
      { kind: "step", id: "c", name: "read", args: "", state: "done", answer: { kind: "object", objectKind: "entity", id: "e1", title: null } },
      { kind: "step", id: "d", name: "provenance", args: "", state: "done", answer: { kind: "claim", lineage: { claimId: "k1", subject: null, predicate: null, value: "v", epistemic: null, supersededBy: null, changesetId: "cs", proposer: null, proposerIsMachine: null, decidedBy: null, rationale: null, evidence: [] } } },
    ];
    const index = citationIndex(v, entries);
    expect(index.get("g9")).toBe("/vaults/v1/documents/s9?page=2&segment=g9");
    expect(index.get("p1")).toBe("/vaults/v1/pages/p1");
    expect(index.get("e1")).toBe("/vaults/v1/entities/e1");
    expect(index.get("k1")).toBe("/vaults/v1/documents/-/claims/k1");
  });

  it("split a reply at its marks, and leave an id nothing returned unresolved", () => {
    const index = new Map([["seg1", "/x"]]);
    const parts = replyParts("It is 412 m [cite:seg1][cite: nope ]. More [cite:", index);
    expect(parts).toEqual([
      { kind: "text", text: "It is 412 m " },
      { kind: "cite", id: "seg1", path: "/x" },
      { kind: "cite", id: "nope", path: null },
      { kind: "text", text: ". More [cite:" },
    ]);
    expect(unresolvedCount(parts)).toBe(1);
    // Hostile text stays text: nothing here is markup.
    expect(replyParts('<img src="https://evil.example/x.png"> [javascript:alert(1)](x)', index)).toEqual([{ kind: "text", text: '<img src="https://evil.example/x.png"> [javascript:alert(1)](x)' }]);
  });
});

describe("a reply prepared for Markdown", () => {
  it("puts a placeholder for each citation, and replaces any the model wrote itself", () => {
    const got = prepareReply(`**Yes** [cite:a]. Forged ${INLINE_OPEN}0${INLINE_CLOSE} [cite:b]`, new Map([["a", "/p"]]));
    expect(got.text).toBe(`**Yes** ${INLINE_OPEN}0${INLINE_CLOSE}. Forged \uFFFD0\uFFFD ${INLINE_OPEN}1${INLINE_CLOSE}`);
    expect(got.cites).toEqual([{ kind: "cite", id: "a", path: "/p" }, { kind: "cite", id: "b", path: null }]);
  });
});

describe("what a person can attach", () => {
  it("offers the object the screen shows, and nothing for a list", () => {
    expect(attachableFrom("documents", ["s1"])).toEqual({ kind: "source", id: "s1" });
    expect(attachableFrom("documents", [])).toBeNull();
    expect(attachableFrom("documents", ["-", "claims", "c1"])).toBeNull();
    expect(attachableFrom("pages", ["p1"])).toEqual({ kind: "page", id: "p1" });
    expect(attachableFrom("entities", ["e1"])).toEqual({ kind: "entity", id: "e1" });
    expect(attachableFrom("entities", ["-", "types", "Person"])).toBeNull();
    expect(attachableFrom("review", ["cs1"])).toEqual({ kind: "review item", id: "cs1" });
    expect(attachableFrom("search", ["x"])).toBeNull();
    expect(messageWith("What is this?", { kind: "source", id: "s1" })).toBe("What is this?\n\n(I'm looking at the source with id s1.)");
    expect(messageWith("Hi", null)).toBe("Hi");
  });
});
