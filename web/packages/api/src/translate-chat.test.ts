import { describe, expect, it } from "vitest";
import { agentEventFromWire, chatSettingsFromWire, chatThreadDetailFromWire, toolAnswerFromWire } from "./translate-chat";

const answer = (data: unknown) => JSON.stringify({ notice: "data is vault content", trust_tiers: [], data });

describe("AG-UI events from the wire", () => {
  // edge-agui's tests read every stream spec 049's backend produced through this. These are single frames.
  it("reads why a run stopped, and a tool's answer inside its result", () => {
    expect(agentEventFromWire({ result: { stop: "max_tokens" }, runId: "run-1", threadId: "thread-1", type: "RUN_FINISHED" })).toEqual({ type: "run_finished", runId: "run-1", stop: "max_tokens" });
    expect(agentEventFromWire({ runId: "r", threadId: "t", type: "RUN_FINISHED" })).toEqual({ type: "run_finished", runId: "r", stop: null });
    const proposed = agentEventFromWire({ content: answer({ proposal_id: "p", state: "pending", refused_by_validators: [] }), messageId: "m", role: "tool", toolCallId: "t0", type: "TOOL_CALL_RESULT" });
    expect(proposed.type === "tool_result" && proposed.answer.kind).toBe("proposal");
    expect(agentEventFromWire({ type: "TEXT_MESSAGE_CONTENT", messageId: "m", delta: "hi" })).toEqual({ type: "text_delta", messageId: "m", delta: "hi" });
  });

  it("turns a frame with missing or wrong fields into unknown, and never throws", () => {
    for (const raw of [null, 3, "RUN_STARTED", [], {}, { type: "STATE_DELTA" }, { type: "TEXT_MESSAGE_CONTENT", messageId: "m", delta: 4 }, { type: "TOOL_CALL_START" }, { type: "TOOL_CALL_RESULT", toolCallId: "t", content: { x: 1 } }]) {
      expect(agentEventFromWire(raw).type).toBe("unknown");
    }
    expect(agentEventFromWire({ type: "RUN_ERROR" })).toEqual({ type: "run_error", message: "The run failed.", code: null });
    expect(agentEventFromWire({ type: "RUN_ERROR", message: "Request aborted", code: "abort" })).toEqual({ type: "run_error", message: "Request aborted", code: "abort" });
  });
});

describe("tool answers", () => {
  it("reads a search with its citations", () => {
    const got = toolAnswerFromWire(
      answer({
        path: "natural",
        low_relevance: false,
        notices: [],
        degraded: [],
        budget: { limit: 1, used: 1, truncated: false },
        items: [{ id: "c1", kind: "claim", text: "t", trust_tiers: [], citations: [{ anchor: "a", rects: [{ page: 2, x0: 1, y0: 2, x1: 3, y1: 4 }], source_id: "s1", segment_id: "g1", evidence_id: "e1" }], conflicts: [] }],
      }),
    );
    expect(got.kind).toBe("search");
    if (got.kind === "search") expect(got.items[0]?.citations[0]).toMatchObject({ sourceId: "s1", segmentId: "g1", evidenceId: "e1" });
  });

  it("reads a segment, a lineage, a proposal and an error", () => {
    expect(toolAnswerFromWire(answer({ id: "g1", source_id: "s1", text: "x", start: 0, end: 1, page_from: 3, t_start_ms: 1000, t_end_ms: 2000 }))).toMatchObject({ kind: "segment", segment: { id: "g1", sourceId: "s1", pageFrom: 3, tStartMs: 1000 } });
    const lineage = toolAnswerFromWire(answer({ assertion: { id: "c9", value: "v" }, changeset_id: "cs", evidence: [] }));
    expect(lineage).toMatchObject({ kind: "claim", lineage: { claimId: "c9", changesetId: "cs" } });
    expect(toolAnswerFromWire(answer({ proposal_id: "p1", state: "pending", refused_by_validators: [{ validator: "quote", message: "not in the source" }] }))).toEqual({ kind: "proposal", proposalId: "p1", state: "pending", refused: ["not in the source"] });
    expect(toolAnswerFromWire(JSON.stringify({ error: { code: "invalid", message: "no tool called fetch_url" } }))).toEqual({ kind: "error", code: "invalid", message: "no tool called fetch_url" });
    expect(toolAnswerFromWire(JSON.stringify({ error: "Tool propose not found" }))).toEqual({ kind: "error", code: null, message: "Tool propose not found" });
    expect(toolAnswerFromWire(answer({ id: "p", slug: "s", title: "A page" }))).toEqual({ kind: "object", objectKind: "page", id: "p", title: "A page" });
  });

  it("calls anything else other, including hostile or broken answers", () => {
    for (const content of ["", "not json", "[]", "null", JSON.stringify({ data: "x" }), answer({ items: "x", path: "natural" }), answer({ html: "<script>alert(1)</script>" })]) {
      expect(toolAnswerFromWire(content)).toEqual({ kind: "other" });
    }
  });
});

describe("settings and threads", () => {
  it("keeps the endpoint null when the server leaves it out", () => {
    expect(chatSettingsFromWire({ enabled: true, day_tokens_used: 5, day_tokens_limit: 10, model: "m" })).toEqual({ enabled: true, threadTokenBudget: null, dayTokenBudget: null, dayTokensUsed: 5, dayTokensLimit: 10, model: "m", endpoint: null });
  });

  it("reads a tool's stored answer the same way as a streamed one", () => {
    const detail = chatThreadDetailFromWire({
      thread: { id: "t", title: "T", created_at_ms: 1, updated_at_ms: 2, tokens: 3 },
      messages: [
        { seq: 1, id: "m1", run_id: "r", role: "user", text: "q", at_ms: 1 },
        { seq: 2, id: "m2", run_id: "r", role: "assistant", text: "", tool_call: { id: "c", name: "propose", args: "{}" }, at_ms: 1 },
        { seq: 3, id: "m3", run_id: "r", role: "tool", text: answer({ proposal_id: "p1", state: "pending", refused_by_validators: [] }), tool_call: { id: "c", name: "propose", args: "{}" }, at_ms: 1 },
        { seq: 4, id: "m4", run_id: "r", role: "assistant", text: "Half", stop: "cancelled", at_ms: 1 },
      ],
    });
    expect(detail.messages.map((m) => m.answer?.kind ?? null)).toEqual([null, null, "proposal", null]);
    expect(detail.messages[3]?.stop).toBe("cancelled");
  });
});
