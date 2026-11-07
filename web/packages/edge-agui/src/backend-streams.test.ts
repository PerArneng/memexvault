// Spec 049's item 8: every stream the backend's scenarios produced, against @ag-ui/core's own schemas. The Rust tests hold these files to what the routes really send.
import { readdirSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { EventSchemas } from "@ag-ui/core/schemas";
import { describe, expect, it } from "vitest";

const folder = join(dirname(fileURLToPath(import.meta.url)), "../../../../crates/memex-http/tests/fixtures/agui");
const files = readdirSync(folder).filter((f) => f.endsWith(".jsonl"));
const read = (file: string) =>
  readFileSync(join(folder, file), "utf8")
    .split("\n")
    .filter(Boolean)
    .map((line) => JSON.parse(line) as Record<string, unknown>);

/** The protocol's order, which the schemas can't see because each judges one event. */
function orderProblems(events: Record<string, unknown>[]): string[] {
  const problems: string[] = [];
  const types = events.map((e) => e.type);
  if (types[0] !== "RUN_STARTED") problems.push("the first event isn't RUN_STARTED");
  const ends = types.filter((t) => t === "RUN_FINISHED" || t === "RUN_ERROR");
  if (ends.length !== 1) problems.push(`${ends.length} ends`);
  if (ends[0] !== types.at(-1)) problems.push("something follows the end");
  let open: unknown = null;
  const seenMessages = new Set<unknown>();
  const called = new Map<unknown, string>();
  for (const e of events) {
    switch (e.type) {
      case "TEXT_MESSAGE_START":
        if (open !== null) problems.push("a message started inside another");
        if (seenMessages.has(e.messageId)) problems.push("a message id came twice");
        seenMessages.add(e.messageId);
        open = e.messageId;
        break;
      case "TEXT_MESSAGE_CONTENT":
        if (open !== e.messageId) problems.push("content outside its message");
        break;
      case "TEXT_MESSAGE_END":
        if (open !== e.messageId) problems.push("an end with no start");
        open = null;
        break;
      case "TOOL_CALL_START":
        if (open !== null) problems.push("a tool call started inside a message");
        if (called.has(e.toolCallId)) problems.push("a tool call id came twice");
        called.set(e.toolCallId, "started");
        break;
      case "TOOL_CALL_ARGS":
        if (called.get(e.toolCallId) !== "started") problems.push("arguments out of order");
        JSON.parse(e.delta as string);
        break;
      case "TOOL_CALL_END":
        if (called.get(e.toolCallId) !== "started") problems.push("an end out of order");
        called.set(e.toolCallId, "ended");
        break;
      case "TOOL_CALL_RESULT":
        if (called.get(e.toolCallId) !== "ended") problems.push("a result out of order");
        called.set(e.toolCallId, "answered");
        break;
      case "RUN_FINISHED":
        if (open !== null) problems.push("it finished with a message open");
        break;
    }
  }
  return problems;
}

describe("the backend's AG-UI streams", () => {
  it("covers the scripted scenarios, the real ADK runner and provider failures", () => {
    for (const name of ["text", "says_nothing", "search_then_answer", "propose", "provider_unavailable", "provider_refused", "budget_mid_run", "budget_before_any_call", "cut_at_its_limit", "too_many_calls"]) {
      expect(files).toContain(`${name}.jsonl`);
    }
    for (const name of ["text", "tool", "refused", "server_error", "rate_limited", "cut_mid_stream", "error_in_a_chunk", "down", "cut_at_its_limit", "turns"]) {
      expect(files).toContain(`adk_${name}.jsonl`);
    }
  });

  it.each(files)("%s passes @ag-ui/core's schemas, in order", (file) => {
    const events = read(file);
    expect(events.length).toBeGreaterThan(0);
    for (const event of events) {
      const parsed = EventSchemas.safeParse(event);
      expect(parsed.success, `${JSON.stringify(event)}: ${parsed.error?.message}`).toBe(true);
    }
    expect(orderProblems(events)).toEqual([]);
  });

  it("tells a bad stream from a good one, so the checks above mean something", () => {
    expect(EventSchemas.safeParse({ type: "TEXT_MESSAGE_CONTENT", messageId: "m" }).success).toBe(false);
    expect(EventSchemas.safeParse({ type: "NOT_AN_EVENT" }).success).toBe(false);
    expect(orderProblems([{ type: "RUN_FINISHED" }, { type: "RUN_STARTED" }])).not.toEqual([]);
    expect(orderProblems([{ type: "RUN_STARTED" }, { type: "TEXT_MESSAGE_START", messageId: "m" }, { type: "RUN_FINISHED" }])).not.toEqual([]);
  });
});
