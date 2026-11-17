import type { EpochMs } from "./index";
import type { ClaimLineage, Segment } from "./documents";
import type { EvidenceItem } from "./search";

/** A vault's chat, as `GET .../chat/settings` says. */
export interface ChatSettings {
  readonly enabled: boolean;
  readonly threadTokenBudget: number | null;
  readonly dayTokenBudget: number | null;
  readonly dayTokensUsed: number;
  readonly dayTokensLimit: number;
  /** Null when the server has no chat model at all. */
  readonly model: string | null;
  /** Only a vault admin is told this. */
  readonly endpoint: string | null;
}

export interface ChatThread {
  readonly id: string;
  readonly title: string;
  readonly createdAt: EpochMs;
  readonly updatedAt: EpochMs;
  readonly tokens: number;
}

/** What one tool answered, read from its JSON. Anything it can't place is `other`, and is still shown as a step. */
export type ChatToolAnswer =
  | { readonly kind: "search"; readonly items: readonly EvidenceItem[]; readonly notices: readonly string[] }
  | { readonly kind: "segment"; readonly segment: Segment }
  | { readonly kind: "claim"; readonly lineage: ClaimLineage }
  | { readonly kind: "proposal"; readonly proposalId: string; readonly state: string; readonly refused: readonly string[] }
  | { readonly kind: "object"; readonly objectKind: string; readonly id: string; readonly title: string | null }
  | { readonly kind: "error"; readonly code: string | null; readonly message: string }
  | { readonly kind: "other" };

/** A message the server kept, from `GET .../chat/threads/{tid}`. */
export interface ChatStoredMessage {
  readonly seq: number;
  readonly id: string;
  readonly runId: string;
  readonly role: "user" | "assistant" | "tool";
  readonly text: string;
  readonly toolCall: { readonly id: string; readonly name: string; readonly args: string } | null;
  /** A tool's answer, read, when this is one. */
  readonly answer: ChatToolAnswer | null;
  readonly stop: string | null;
  readonly at: EpochMs;
}

export interface ChatThreadDetail {
  readonly thread: ChatThread;
  readonly messages: readonly ChatStoredMessage[];
  readonly nextCursor: string | null;
}

/** The AG-UI events the transcript understands. Anything else is `unknown`, and changes nothing. */
export type AgentEvent =
  | { readonly type: "run_started"; readonly runId: string; readonly threadId: string }
  | { readonly type: "run_finished"; readonly runId: string; readonly stop: string | null }
  | { readonly type: "run_error"; readonly message: string; readonly code: string | null }
  | { readonly type: "text_start"; readonly messageId: string }
  | { readonly type: "text_delta"; readonly messageId: string; readonly delta: string }
  | { readonly type: "text_end"; readonly messageId: string }
  | { readonly type: "tool_start"; readonly toolCallId: string; readonly name: string }
  | { readonly type: "tool_args"; readonly toolCallId: string; readonly delta: string }
  | { readonly type: "tool_end"; readonly toolCallId: string }
  | { readonly type: "tool_result"; readonly toolCallId: string; readonly answer: ChatToolAnswer }
  | { readonly type: "unknown"; readonly name: string };

/** What a channel tells its listener about one run. */
export type ChannelEvent =
  | { readonly kind: "event"; readonly event: AgentEvent }
  /** The server refused the run before a stream opened, such as chat being off or a run already going in the thread. */
  | { readonly kind: "refused"; readonly status: number; readonly code: string | null; readonly message: string }
  /** The connection broke, or the stream couldn't be read. */
  | { readonly kind: "dropped"; readonly message: string }
  /** The stream ended. Always the last thing said. */
  | { readonly kind: "closed" };
