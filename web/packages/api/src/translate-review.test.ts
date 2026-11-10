import { describe, expect, it } from "vitest";
import { batchFromWire, changesetFromWire, reviewDetailFromWire, reviewFilterToWire } from "./translate-review";
import type { components } from "./generated/schema";

const wire: components["schemas"]["ChangeSetDto"] = { id: "c", state: "pending", proposer: "a", proposer_is_machine: true, rationale: "r", idempotency_key: "k", ops: [], validators: [], risk: "review", proposed_at_ms: 5 };

describe("review wire to domain", () => {
  it("fills what's absent with null or empty", () => {
    expect(changesetFromWire(wire)).toMatchObject({ id: "c", decidedBy: null, requestedBy: null, itemTypes: [], originalOps: null, reverts: null });
  });
  it("keeps who decided, and a chat's person", () => {
    expect(changesetFromWire({ ...wire, decided_by: "p", decision_reason: "why", decided_at_ms: 9, requested_by: "sam" })).toMatchObject({ decidedBy: "p", decisionReason: "why", decidedAt: 9, requestedBy: "sam" });
  });
  it("reads an item's evidence, with its rects and flags, and a flag it can't read still shows as a flag", () => {
    const detail = reviewDetailFromWire({
      changeset: wire,
      combinations: [],
      needs_a_person: true,
      evidence: [{ claim_id: "x", segment_id: "s", quote: "q", superseded: false, page_flags: [{ flag: "hidden_text", words: 2 } as never, 5 as never], rects: [{ page: 1, x0: 1, y0: 2, x1: 3, y1: 4 }] }],
    });
    expect(detail.needsAPerson).toBe(true);
    expect(detail.verdict).toBeNull();
    expect(detail.evidence[0]).toMatchObject({ quote: "q", sourceId: null, rects: [{ page: 1, x0: 1, y0: 2, x1: 3, y1: 4 }], pageFlags: [{ kind: "hidden_text", words: 2 }, { kind: "other", name: "unknown" }] });
  });
  it("reads a batch's failures", () => {
    expect(batchFromWire({ results: [{ id: "a", error_code: "conflict", error_message: "sam" }, { id: "b", changeset: wire }] })).toMatchObject([{ id: "a", changeset: null, errorCode: "conflict" }, { id: "b", errorCode: null, changeset: { id: "c" } }]);
  });
  it("sends only the filters that are set", () => {
    expect(reviewFilterToWire({ extractor: "", promptVersion: "p", itemType: "", proposer: "", verdict: "none", sourceId: "" })).toEqual({ prompt_version: "p", verdict: "none" });
  });
});
