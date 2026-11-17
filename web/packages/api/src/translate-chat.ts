import { epochMs } from "@memex/types";
import type { AgentEvent, ChatSettings, ChatStoredMessage, ChatThread, ChatThreadDetail, ChatToolAnswer, Page } from "@memex/types";
import type { components } from "./generated/schema";
import { claimLineageFromWire, segmentFromWire } from "./translate-documents";
import { evidencePackageFromWire } from "./translate-search";

type Schemas = components["schemas"];
type Json = Readonly<Record<string, unknown>>;

const isRecord = (value: unknown): value is Json => typeof value === "object" && value !== null && !Array.isArray(value);
const text = (value: unknown): string | null => (typeof value === "string" ? value : null);

export const chatSettingsFromWire = (s: Schemas["ChatSettingsDto"]): ChatSettings => ({
  enabled: s.enabled,
  threadTokenBudget: s.thread_token_budget ?? null,
  dayTokenBudget: s.day_token_budget ?? null,
  dayTokensUsed: s.day_tokens_used,
  dayTokensLimit: s.day_tokens_limit,
  model: s.model ?? null,
  endpoint: s.endpoint ?? null,
});

export const chatThreadFromWire = (t: Schemas["ChatThreadDto"]): ChatThread => ({
  id: t.id,
  title: t.title,
  createdAt: epochMs(t.created_at_ms),
  updatedAt: epochMs(t.updated_at_ms),
  tokens: t.tokens,
});

export const chatThreadPageFromWire = (p: Schemas["ChatThreadPage"]): Page<ChatThread> => ({ items: p.items.map(chatThreadFromWire), nextCursor: p.next_cursor ?? null });

const objectKind = (data: Json): string => {
  if ("slug" in data || "body" in data || "blocks" in data) return "page";
  if ("fields" in data || "entity_type" in data) return "entity";
  if ("sha256" in data || "media_type" in data) return "source";
  return "object";
};

/**
 * Reads a tool's answer, `{notice, trust_tiers, data}` or `{error}`, by its shape rather than the tool's name, so history and the stream
 * read it the same way. Nothing in it is trusted: what doesn't fit a known shape is `other`, and a translator that throws on a bad shape
 * makes it `other` too.
 */
export const toolAnswerFromWire = (content: string): ChatToolAnswer => {
  let parsed: unknown;
  try {
    parsed = JSON.parse(content);
  } catch {
    return { kind: "other" };
  }
  if (!isRecord(parsed)) return { kind: "other" };
  if (isRecord(parsed.error)) return { kind: "error", code: text(parsed.error.code), message: text(parsed.error.message) ?? "The tool failed." };
  const data = parsed.data;
  if (!isRecord(data)) return { kind: "other" };
  try {
    if (Array.isArray(data.items) && typeof data.path === "string") {
      const pkg = evidencePackageFromWire(data as unknown as Schemas["EvidencePackageDto"]);
      return { kind: "search", items: pkg.items, notices: pkg.notices };
    }
    if (typeof data.proposal_id === "string") {
      const refused = Array.isArray(data.refused_by_validators) ? data.refused_by_validators : [];
      return {
        kind: "proposal",
        proposalId: data.proposal_id,
        state: text(data.state) ?? "unknown",
        refused: refused.map((r) => (isRecord(r) ? (text(r.message) ?? text(r.validator) ?? "refused") : String(r))),
      };
    }
    if (typeof data.changeset_id === "string" && Array.isArray(data.evidence)) {
      const assertion = isRecord(data.assertion) ? data.assertion : {};
      return { kind: "claim", lineage: claimLineageFromWire(text(assertion.id) ?? "", data as unknown as Schemas["LineageDto"]) };
    }
    if (typeof data.id === "string" && typeof data.source_id === "string" && typeof data.text === "string" && typeof data.start === "number") {
      return { kind: "segment", segment: segmentFromWire(data as unknown as Schemas["SegmentDto"]) };
    }
    if (typeof data.id === "string") return { kind: "object", objectKind: objectKind(data), id: data.id, title: text(data.title) ?? text(data.name) };
  } catch {
    return { kind: "other" };
  }
  return { kind: "other" };
};

const role = (r: string): ChatStoredMessage["role"] => (r === "user" || r === "tool" ? r : "assistant");

export const chatMessageFromWire = (m: Schemas["ChatMessageDto"]): ChatStoredMessage => ({
  seq: m.seq,
  id: m.id,
  runId: m.run_id,
  role: role(m.role),
  text: m.text,
  toolCall: m.tool_call === null || m.tool_call === undefined ? null : { id: m.tool_call.id, name: m.tool_call.name, args: m.tool_call.args },
  answer: m.role === "tool" ? toolAnswerFromWire(m.text) : null,
  stop: m.stop ?? null,
  at: epochMs(m.at_ms),
});

export const chatThreadDetailFromWire = (d: Schemas["ChatThreadDetail"]): ChatThreadDetail => ({
  thread: chatThreadFromWire(d.thread),
  messages: d.messages.map(chatMessageFromWire),
  nextCursor: d.next_cursor ?? null,
});

/** One AG-UI event, as it arrived. A frame with a type it doesn't know, or the wrong fields for its type, is `unknown`. */
export const agentEventFromWire = (raw: unknown): AgentEvent => {
  if (!isRecord(raw)) return { type: "unknown", name: "" };
  const name = text(raw.type) ?? "";
  const messageId = text(raw.messageId);
  const toolCallId = text(raw.toolCallId);
  switch (name) {
    case "RUN_STARTED":
      return { type: "run_started", runId: text(raw.runId) ?? "", threadId: text(raw.threadId) ?? "" };
    case "RUN_FINISHED": {
      const result = isRecord(raw.result) ? raw.result : {};
      return { type: "run_finished", runId: text(raw.runId) ?? "", stop: text(result.stop) };
    }
    case "RUN_ERROR":
      return { type: "run_error", message: text(raw.message) ?? "The run failed.", code: text(raw.code) };
    case "TEXT_MESSAGE_START":
      return messageId === null ? { type: "unknown", name } : { type: "text_start", messageId };
    case "TEXT_MESSAGE_CONTENT":
      return messageId === null || typeof raw.delta !== "string" ? { type: "unknown", name } : { type: "text_delta", messageId, delta: raw.delta };
    case "TEXT_MESSAGE_END":
      return messageId === null ? { type: "unknown", name } : { type: "text_end", messageId };
    case "TOOL_CALL_START":
      return toolCallId === null ? { type: "unknown", name } : { type: "tool_start", toolCallId, name: text(raw.toolCallName) ?? "tool" };
    case "TOOL_CALL_ARGS":
      return toolCallId === null || typeof raw.delta !== "string" ? { type: "unknown", name } : { type: "tool_args", toolCallId, delta: raw.delta };
    case "TOOL_CALL_END":
      return toolCallId === null ? { type: "unknown", name } : { type: "tool_end", toolCallId };
    case "TOOL_CALL_RESULT":
      return toolCallId === null || typeof raw.content !== "string" ? { type: "unknown", name } : { type: "tool_result", toolCallId, answer: toolAnswerFromWire(raw.content) };
    default:
      return { type: "unknown", name };
  }
};
