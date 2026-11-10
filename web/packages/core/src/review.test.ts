import { epochMs } from "@memex/types";
import { describe, expect, it } from "vitest";
import { afterRemoving, AUDITOR, AUTO_COMMIT_RULES, automaticPath, autoCommitStateText, batchesOf, decisionAccess, decisionAccessText, decisionFailure, describeMutation, describeOp, frameTouchesQueue, INITIAL_QUEUE, markReviewQuote, parseEditedOps, queueCommandFor, queueReconciled, queueStep, selectedIn, sourcePlace, summarizeBatch, validatorLine } from "./review";
import type { QueueState, QueueView } from "./review";
import { changesetWith, combinationWith, detailWith, httpFailure, identityWith, UNREACHABLE } from "./testing";
import { vaultId } from "@memex/types";

const press = (key: string, over = {}) => ({ key, ctrl: false, meta: false, alt: false, typing: false, ...over });
const view = (over: Partial<QueueView> = {}): QueueView => ({ ids: ["a", "b", "c"], canDecide: true, ready: true, evidenceCount: 1, ...over });
const at = (current: string | null, over: Partial<QueueState> = {}): QueueState => ({ ...INITIAL_QUEUE, current, ...over });

describe("which key does what", () => {
  it("maps every documented key", () => {
    expect(["j", "ArrowDown", "k", "ArrowUp", "a", "n", "r", "e", "x", "X", "A", "R", "o", "s", "?", "Escape"].map((k) => queueCommandFor(press(k)))).toEqual([
      "next", "next", "previous", "previous", "approve", "approve_note", "reject", "edit", "toggle_select", "select_all", "batch_approve", "batch_reject", "open_source", "next_evidence", "help", "cancel",
    ]);
  });
  it("tells a capital from a small letter, as they decide different things", () => {
    expect(queueCommandFor(press("a"))).not.toBe(queueCommandFor(press("A")));
  });
  it("never takes typing, or the browser's own shortcuts", () => {
    expect(queueCommandFor(press("a", { typing: true }))).toBeNull();
    expect(queueCommandFor(press("Escape", { typing: true }))).toBeNull();
    expect(queueCommandFor(press("a", { ctrl: true }))).toBeNull();
    expect(queueCommandFor(press("r", { meta: true }))).toBeNull();
    expect(queueCommandFor(press("j", { alt: true }))).toBeNull();
  });
  it("ignores keys it doesn't know, including ones named like object properties", () => {
    expect(queueCommandFor(press("z"))).toBeNull();
    expect(queueCommandFor(press("constructor"))).toBeNull();
    expect(queueCommandFor(press("toString"))).toBeNull();
  });
});

describe("moving through the queue", () => {
  it("goes forward and back, and stops at both ends", () => {
    expect(queueStep(at("a"), "next", view()).state.current).toBe("b");
    expect(queueStep(at("c"), "next", view()).state.current).toBe("c");
    expect(queueStep(at("b"), "previous", view()).state.current).toBe("a");
    expect(queueStep(at("a"), "previous", view()).state.current).toBe("a");
  });
  it("starts at the first item from nowhere, or from one that's gone", () => {
    expect(queueStep(at(null), "next", view()).state.current).toBe("a");
    expect(queueStep(at("gone"), "next", view()).state.current).toBe("a");
  });
  it("an empty queue has nowhere to go", () => {
    expect(queueStep(at("a"), "next", view({ ids: [] })).state.current).toBeNull();
  });
  it("moving starts the new item at its first quote", () => {
    expect(queueStep(at("a", { evidence: 2 }), "next", view()).state.evidence).toBe(0);
  });
  it("cycles quotes, and stays put with fewer than two", () => {
    const two = view({ evidenceCount: 2 });
    const once = queueStep(at("a"), "next_evidence", two).state;
    expect(once.evidence).toBe(1);
    expect(queueStep(once, "next_evidence", two).state.evidence).toBe(0);
    expect(queueStep(at("a"), "next_evidence", view()).state.evidence).toBe(0);
  });
});

describe("deciding by key", () => {
  it("approves what's on screen", () => {
    expect(queueStep(at("a"), "approve", view()).approve).toBe(true);
  });
  it("won't decide what hasn't been seen", () => {
    const out = queueStep(at("a"), "approve", view({ ready: false }));
    expect(out).toMatchObject({ approve: false, refused: "not_loaded" });
    expect(queueStep(at("a"), "reject", view({ ready: false })).refused).toBe("not_loaded");
  });
  it("won't decide with no right to, or nothing there", () => {
    for (const command of ["approve", "approve_note", "reject", "edit"] as const) {
      const out = queueStep(at("a"), command, view({ canDecide: false }));
      expect(out.refused).toBe("cant_decide");
      expect(out.approve).toBe(false);
      expect(out.state.panel.kind).toBe("none");
      expect(queueStep(at(null), command, view()).refused).toBe("nothing_here");
    }
  });
  it("opens a form for what takes words", () => {
    expect(queueStep(at("a"), "reject", view()).state.panel.kind).toBe("reject");
    expect(queueStep(at("a"), "approve_note", view()).state.panel.kind).toBe("approve_note");
    expect(queueStep(at("a"), "edit", view()).state.panel.kind).toBe("edit");
  });
  it("while a form is up, no other command acts, so a stray key can't decide", () => {
    const open = at("a", { panel: { kind: "reject" } });
    for (const command of ["approve", "next", "edit", "toggle_select", "batch_approve"] as const) {
      const out = queueStep(open, command, view());
      expect(out.approve).toBe(false);
      expect(out.state).toBe(open);
    }
  });
  it("Esc closes the form, and then clears the selection", () => {
    const out = queueStep(at("a", { panel: { kind: "reject" }, selected: ["a"] }), "cancel", view());
    expect(out.state.panel.kind).toBe("none");
    expect(out.state.selected).toEqual(["a"]);
    expect(queueStep(out.state, "cancel", view()).state.selected).toEqual([]);
  });
  it("help opens and closes, and another command closes it and acts", () => {
    const open = queueStep(at("a"), "help", view()).state;
    expect(open.panel.kind).toBe("help");
    expect(queueStep(open, "help", view()).state.panel.kind).toBe("none");
    const moved = queueStep(open, "next", view()).state;
    expect(moved.panel.kind).toBe("none");
    expect(moved.current).toBe("b");
  });
});

describe("batches", () => {
  it("selects and unselects", () => {
    const one = queueStep(at("a"), "toggle_select", view()).state;
    expect(one.selected).toEqual(["a"]);
    expect(queueStep(one, "toggle_select", view()).state.selected).toEqual([]);
  });
  it("selects everything shown, then nothing", () => {
    const all = queueStep(at("a"), "select_all", view()).state;
    expect(all.selected).toEqual(["a", "b", "c"]);
    expect(queueStep(all, "select_all", view()).state.selected).toEqual([]);
  });
  it("refuses a batch of nothing, or by someone who can't decide", () => {
    expect(queueStep(at("a"), "batch_approve", view()).refused).toBe("nothing_selected");
    expect(queueStep(at("a"), "batch_reject", view()).refused).toBe("nothing_selected");
    expect(queueStep(at("a", { selected: ["a"] }), "batch_approve", view({ canDecide: false })).refused).toBe("cant_decide");
  });
  it("a selection that has left the list can't be batched", () => {
    const gone = at("a", { selected: ["x"] });
    expect(selectedIn(gone, ["a"])).toEqual([]);
    expect(queueStep(gone, "batch_approve", view({ ids: ["a"] })).refused).toBe("nothing_selected");
  });
  it("keeps the list's order, whatever order they were picked in", () => {
    expect(selectedIn(at("a", { selected: ["c", "a"] }), ["a", "b", "c"])).toEqual(["a", "c"]);
  });
  it("splits a long selection at the server's limit", () => {
    expect(batchesOf(Array.from({ length: 450 }, (_, i) => i)).map((b) => b.length)).toEqual([200, 200, 50]);
    expect(batchesOf([])).toEqual([]);
  });
  it("sums up what went through and what didn't", () => {
    expect(summarizeBatch([{ id: "a", errorCode: null, errorMessage: null }, { id: "b", errorCode: "conflict", errorMessage: "pat decided" }, { id: "c", errorCode: "busy", errorMessage: null }])).toEqual({
      done: 1,
      problems: [{ id: "b", message: "pat decided" }, { id: "c", message: "busy" }],
    });
  });
});

describe("after something leaves the list", () => {
  it("goes to the next one, going forward", () => {
    expect(afterRemoving(["a", "b", "c"], ["b"], "b")).toBe("c");
    expect(afterRemoving(["a", "b", "c"], ["a", "b"], "a")).toBe("c");
  });
  it("falls back to the last one at the end", () => {
    expect(afterRemoving(["a", "b", "c"], ["c"], "c")).toBe("b");
  });
  it("is nothing when nothing is left", () => {
    expect(afterRemoving(["a"], ["a"], "a")).toBeNull();
  });
  it("stays put when the item in view isn't the one leaving", () => {
    expect(afterRemoving(["a", "b"], ["b"], "a")).toBe("a");
  });
  it("a refetch that drops the item in view moves forward, and drops it from the selection", () => {
    const out = queueReconciled(at("b", { selected: ["b", "c"] }), ["a", "b", "c"], ["a", "c"]);
    expect(out.current).toBe("c");
    expect(out.selected).toEqual(["c"]);
  });
  it("new items arriving disturb nothing", () => {
    const state = at("b", { selected: ["b"] });
    const out = queueReconciled(state, ["a", "b"], ["a", "b", "c"]);
    expect(out).toBe(state);
  });
  it("the first read lands on the first item, unless a link named one that's there", () => {
    expect(queueReconciled(at(null), [], ["a", "b"]).current).toBe("a");
    expect(queueReconciled(at("b"), [], ["a", "b"]).current).toBe("b");
  });
  it("an empty list leaves nothing in view", () => {
    expect(queueReconciled(at("a"), ["a"], []).current).toBeNull();
  });
});

describe("what a failed decision means", () => {
  it("a conflict keeps the server's words, which name who decided", () => {
    expect(decisionFailure(httpFailure(409, "conflict", "sam already decided this"))).toEqual({ kind: "conflict", message: "sam already decided this" });
    expect(decisionFailure(httpFailure(409, "stale_version", "moved")).kind).toBe("conflict");
  });
  it("tells the other answers apart", () => {
    expect(decisionFailure(httpFailure(403, "agent_session", "no")).kind).toBe("not_allowed");
    expect(decisionFailure(httpFailure(400, "invalid", "no")).kind).toBe("invalid");
    expect(decisionFailure(httpFailure(500, "internal")).kind).toBe("failed");
    expect(decisionFailure(UNREACHABLE).kind).toBe("failed");
  });
});

describe("who may decide", () => {
  const vault = vaultId("v");
  const grant = (scope: "read" | "propose" | "admin") => [{ vault, scope }] as const;
  it("a person with admin scope", () => {
    expect(decisionAccess(identityWith(grant("admin")), vault).kind).toBe("allowed");
    expect(decisionAccess(identityWith([], true), vault).kind).toBe("allowed");
  });
  it("not a person with less", () => {
    expect(decisionAccess(identityWith(grant("propose")), vault)).toEqual({ kind: "needs_admin", have: "propose" });
    expect(decisionAccess(identityWith([]), vault)).toEqual({ kind: "needs_admin", have: "no" });
  });
  it("never an agent, whatever its scope, and it says so", () => {
    const base = identityWith(grant("admin"), true);
    const agent = { ...base, principal: { ...base.principal, machine: true } };
    const access = decisionAccess(agent, vault);
    expect(access.kind).toBe("agent");
    expect(decisionAccessText(access)).toContain("agents can't approve");
  });
  it("says nothing to someone who can", () => {
    expect(decisionAccessText({ kind: "allowed" })).toBeNull();
  });
});

describe("the edit box", () => {
  it("takes a list of ops", () => {
    expect(parseEditedOps('[{"op":"edit_page","id":"p"}]')).toEqual({ ok: true, ops: [{ op: "edit_page", id: "p" }] });
  });
  it("refuses what can't be ops, and says why", () => {
    for (const text of ["not json", "{}", "[]", '[1]', '[{"id":"p"}]', '[null]', "[[]]"]) expect(parseEditedOps(text).ok).toBe(false);
  });
});

describe("describing a change, as plain data", () => {
  it("names the kinds people decide on", () => {
    expect(describeOp({ op: "add_assertion", subject: "s", predicate: "p", value: 1, epistemic: "asserted" })).toEqual({
      kind: "add_assertion", title: "Add a claim", epistemic: "asserted", fields: [{ label: "Subject", value: "s" }, { label: "Predicate", value: "p" }, { label: "Value", value: "1" }],
    });
    expect(describeOp({ op: "delete_page", id: "p" }).title).toBe("Delete a page");
    expect(describeOp({ op: "retract_assertion", target: "c" }).title).toBe("Retire a claim");
  });
  it("shows a replacement's claim and what it replaces", () => {
    const out = describeOp({ op: "supersede_assertion", target: "old", replacement: { op: "add_assertion", subject: "s", value: "new", epistemic: "asserted" } });
    expect(out.fields).toContainEqual({ label: "Target", value: "old" });
    expect(out.fields).toContainEqual({ label: "Value", value: "new" });
  });
  it("an unknown or broken op still reads as something, and passes text through untouched", () => {
    expect(describeOp({ op: "new_thing" }).title).toBe("new thing");
    expect(describeOp(null).kind).toBe("unknown");
    expect(describeOp({ op: "add_assertion", subject: "<img src=x>" }).fields[0]?.value).toBe("<img src=x>");
  });
  it("reads an inverse's writes and removals", () => {
    expect(describeMutation({ put: { entity_type: {} } })).toBe("Write back: entity type");
    expect(describeMutation({ delete: { table: "assertion", id: "c" } })).toBe("Remove: assertion c");
    expect(describeMutation(5)).toContain("doesn't know");
  });
  it("a validator result it can't read is never shown as a pass", () => {
    expect(validatorLine({ validator: "schema", passed: true, detail: null })).toEqual({ name: "schema", passed: true, detail: "" });
    expect(validatorLine({ validator: "quote", passed: false, detail: "not found" })).toEqual({ name: "quote", passed: false, detail: "not found" });
    expect(validatorLine({ weird: 1 }).passed).toBe(false);
    expect(validatorLine("nope").passed).toBe(false);
  });
});

describe("the automatic path shown on an item", () => {
  const machine = changesetWith({ id: "c", itemTypes: ["claim"] });
  it("a chat's proposal never has one, and adds no samples, whatever is earned", () => {
    const path = automaticPath(detailWith(changesetWith({ id: "c", requestedBy: "sam", itemTypes: ["claim"] }), { needsAPerson: true, autoCommitBlock: "it came from a chat", combinations: [combinationWith({ on: true })] }));
    expect(path.tone).toBe("never");
    expect(path.detail).toContain("never auto-commit");
    expect(path.detail).toContain("no earned samples");
  });
  it("what always needs a person never has one, whatever is earned", () => {
    const path = automaticPath(detailWith(machine, { needsAPerson: true, combinations: [combinationWith({ on: true })] }));
    expect(path.tone).toBe("never");
    expect(path.title).toBe("Only a person can approve this.");
  });
  it("a source below trusted never has one, whatever is earned", () => {
    const path = automaticPath(detailWith(machine, { autoCommitBlock: "it cites a source below trusted", combinations: [combinationWith({ on: true })] }));
    expect(path.tone).toBe("never");
    expect(path.detail).toContain("it cites a source below trusted");
    expect(path.detail).toContain("marked trusted");
  });
  it("a person's own proposal isn't anyone's to auto-commit", () => {
    expect(automaticPath(detailWith(changesetWith({ id: "c", proposerIsMachine: false }))).tone).toBe("never");
  });
  it("otherwise it says where the combination stands, only as far as every type has earned it", () => {
    expect(automaticPath(detailWith(machine)).tone).toBe("off");
    expect(automaticPath(detailWith(machine, { combinations: [combinationWith({ on: true })] })).tone).toBe("on");
    const two = changesetWith({ id: "c", itemTypes: ["claim", "page"] });
    expect(automaticPath(detailWith(two, { combinations: [combinationWith({ on: true })] })).tone).toBe("off");
  });
});

describe("the words for a combination's state", () => {
  const change = (kind: string, by: string) => ({ at: epochMs(1), by, kind, reason: "r", correct: 5, incorrect: 0 });
  it("on says only trusted sources count", () => {
    const text = autoCommitStateText(combinationWith({ on: true, wilsonBound: 0.96 }), 0.95);
    expect(text).toContain("96.0%");
    expect(text).toContain("95.0%");
    expect(text).toContain("marked trusted");
  });
  it("one failed audit is said plainly, and is told from a person's disable", () => {
    const audited = autoCommitStateText(combinationWith({ history: [change("disabled", AUDITOR)] }), 0.95);
    expect(audited).toContain("an audit failed");
    expect(audited).toContain("One failed audit");
    const person = autoCommitStateText(combinationWith({ history: [change("disabled", "sam")] }), 0.95);
    expect(person).toContain("sam turned it off");
    expect(person).toContain("no way to turn it on by hand");
  });
  it("has the case with no samples, and the case of falling short", () => {
    expect(autoCommitStateText(combinationWith(), 0.95)).toContain("no samples yet");
    expect(autoCommitStateText(combinationWith({ correct: 9, incorrect: 1, wilsonBound: 0.596 }), 0.95)).toContain("59.6% is below the 95.0%");
    expect(autoCommitStateText(combinationWith({ correct: 1, wilsonBound: 0.2, history: [change("disabled", AUDITOR)] }), 0.95)).toContain("An audit turned it off");
  });
  it("the rules state each of the user's decisions", () => {
    const rules = AUTO_COMMIT_RULES.join(" ");
    expect(rules).toContain("Only sources a person marked trusted can auto-commit");
    expect(rules).toContain("A chat's proposals never auto-commit");
    expect(rules).toContain("adds no earned samples");
    expect(rules).toContain("One failed audit turns off auto-commit");
  });
});

describe("the source pane", () => {
  const evidence = { claimId: "c", segmentId: "s", quote: "q", sourceId: "src", sourceName: "n", trustTier: "trusted", renditionId: "r", pageFrom: null, pageTo: null, superseded: false, pageFlags: [], rects: [] };
  it("shows a quote on its page where it has one, by its rects first", () => {
    expect(sourcePlace({ ...evidence, pageFrom: 2, rects: [{ page: 3, x0: 0, y0: 0, x1: 1, y1: 1 }] })).toEqual({ kind: "pages", source: "src", page: 3 });
    expect(sourcePlace({ ...evidence, pageFrom: 2 })).toEqual({ kind: "pages", source: "src", page: 2 });
  });
  it("otherwise in its words, including a source that's gone", () => {
    expect(sourcePlace(evidence)).toEqual({ kind: "text", source: "src" });
    expect(sourcePlace({ ...evidence, sourceId: null, pageFrom: 1 })).toEqual({ kind: "text", source: null });
  });
  it("marks a quote, and guesses nothing when it isn't there", () => {
    expect(markReviewQuote("one two three", "two")).toEqual({ before: "one ", mark: "two", after: " three" });
    expect(markReviewQuote("one two", "nine")).toEqual({ before: "one two", mark: "", after: "" });
    expect(markReviewQuote("one", "")).toEqual({ before: "one", mark: "", after: "" });
  });
});

describe("what in the stream moves the queue", () => {
  it("a change to a ChangeSet does, and a frame too full to list them might", () => {
    expect(frameTouchesQueue({ changesets: 1, omitted: 0, kind: "state.recorded" })).toBe(true);
    expect(frameTouchesQueue({ changesets: 0, omitted: 2, kind: "state.recorded" })).toBe(true);
    expect(frameTouchesQueue({ changesets: 0, omitted: 0, kind: "changeset.recorded" })).toBe(true);
  });
  it("other work doesn't", () => {
    expect(frameTouchesQueue({ changesets: 0, omitted: 0, kind: "segment.recorded" })).toBe(false);
  });
});
