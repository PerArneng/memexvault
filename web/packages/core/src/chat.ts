import { INLINE_CLOSE, INLINE_OPEN } from "@memex/types";
import type { ChannelEvent, ChatStoredMessage, ChatToolAnswer, EvidenceItem, VaultId } from "@memex/types";
import { documentPath } from "./document-routes";
import type { SourceViewRoute } from "./document-routes";
import { sectionPath } from "./navigation";
import { citationPath } from "./search";

// ---- The channel

export interface AgentRunRequest {
  readonly vault: VaultId;
  readonly threadId: string;
  readonly runId: string;
  /** Only the newest message goes: the server keeps the thread's history and reads nothing else. */
  readonly messageId: string;
  readonly message: string;
}

export interface AgentRun {
  /** Ends the run from this side. The server takes a closed connection as the cancel. */
  stop(): void;
}

/** One run of the chat agent over AG-UI. Every run ends with `closed`, whatever happened before it. */
export interface AgentChannel {
  run(request: AgentRunRequest, listener: (event: ChannelEvent) => void): AgentRun;
}

// ---- The transcript

/** How a reply ended. Anything but `streaming` and `done` is shown on the reply, so a partial answer never passes for a whole one. */
export type ReplyEnd = "streaming" | "done" | "stopped" | "cut" | "budget" | "failed" | "dropped";

export type TranscriptEntry =
  | { readonly kind: "user"; readonly id: string; readonly text: string }
  | { readonly kind: "reply"; readonly id: string; readonly text: string; readonly end: ReplyEnd }
  | { readonly kind: "step"; readonly id: string; readonly name: string; readonly args: string; readonly state: "running" | "done" | "failed"; readonly answer: ChatToolAnswer | null }
  | { readonly kind: "notice"; readonly id: string; readonly tone: "stopped" | "error" | "info"; readonly text: string };

export type RunStatus = "idle" | "running" | "stopping";

export interface TranscriptState {
  readonly entries: readonly TranscriptEntry[];
  readonly status: RunStatus;
  /** The run this transcript follows. Anything about another run is late, and changes nothing. */
  readonly runId: string | null;
  /** Counts what didn't fit, for tests and for the curious: late events, unknown events and the like. */
  readonly ignored: number;
}

export const EMPTY_TRANSCRIPT: TranscriptState = { entries: [], status: "idle", runId: null, ignored: 0 };

export type TranscriptAction =
  | { readonly type: "sent"; readonly runId: string; readonly messageId: string; readonly text: string }
  | { readonly type: "stop_requested" }
  | { readonly type: "channel"; readonly runId: string; readonly event: ChannelEvent }
  | { readonly type: "loaded"; readonly messages: readonly ChatStoredMessage[] };

const STOP_TEXT: Readonly<Record<string, string>> = {
  max_tokens: "The reply reached its length limit, so it's cut short.",
  thread_budget: "This thread has used its token budget. Start a new thread to go on.",
  day_budget: "This vault's chat has used today's token budget. It resets at midnight UTC.",
  model_calls: "The agent reached its limit on model calls for one question.",
  cancelled: "Stopped.",
};

const endOfStop = (stop: string): ReplyEnd => {
  if (stop === "max_tokens") return "cut";
  if (stop === "thread_budget" || stop === "day_budget") return "budget";
  if (stop === "cancelled") return "stopped";
  if (stop === "failed") return "failed";
  return "cut";
};

const refusalText = (status: number, code: string | null, message: string): string => {
  if (code === "chat_disabled") return "Chat is off for this vault. An admin can turn it on.";
  if (status === 409) return "A reply is still being written in this thread. Wait for it, or stop it, and send again.";
  if (status === 429) return "The server is answering as many chats as it allows. Try again in a moment.";
  if (status === 503) return "This server has no chat model configured, so nothing can answer.";
  if (status === 401) return "Your session has ended. Sign in again to chat.";
  return message === "" ? `The server refused this message (HTTP ${String(status)}).` : `The server refused this message: ${message}`;
};

const replace = (entries: readonly TranscriptEntry[], at: number, entry: TranscriptEntry): readonly TranscriptEntry[] => [...entries.slice(0, at), entry, ...entries.slice(at + 1)];

const findIndex = (entries: readonly TranscriptEntry[], kind: TranscriptEntry["kind"], id: string): number => entries.findIndex((e) => e.kind === kind && e.id === id);

/** Every reply still open, ended the same way. */
const endOpen = (entries: readonly TranscriptEntry[], end: ReplyEnd): readonly TranscriptEntry[] =>
  entries.map((e) => (e.kind === "reply" && e.end === "streaming" ? { ...e, end } : e.kind === "step" && e.state === "running" ? { ...e, state: end === "done" ? "done" : "failed" } : e));

const notice = (state: TranscriptState, tone: "stopped" | "error" | "info", text: string): readonly TranscriptEntry[] => [...state.entries, { kind: "notice", id: `${state.runId ?? "run"}-notice-${String(state.entries.length)}`, tone, text }];

const finish = (state: TranscriptState, end: ReplyEnd, tone: "stopped" | "error" | "info" | null, text: string): TranscriptState => {
  const ended = { ...state, entries: endOpen(state.entries, end), status: "idle" as const };
  return tone === null ? ended : { ...ended, entries: notice(ended, tone, text) };
};

const ignore = (state: TranscriptState): TranscriptState => ({ ...state, ignored: state.ignored + 1 });

const applyEvent = (state: TranscriptState, event: ChannelEvent): TranscriptState => {
  const stopping = state.status === "stopping";
  switch (event.kind) {
    case "refused":
      return finish(state, "failed", "error", refusalText(event.status, event.code, event.message));
    case "dropped":
      return stopping ? finish(state, "stopped", "stopped", "Stopped.") : finish(state, "dropped", "error", "The connection dropped before the reply finished. What arrived is kept, and marked.");
    case "closed":
      // A stream that ends with no end event was cut off somewhere on the way.
      return stopping ? finish(state, "stopped", "stopped", "Stopped.") : finish(state, "dropped", "error", "The reply ended without saying it had finished, so what arrived may be incomplete.");
    case "event":
      break;
  }
  const e = event.event;
  const { entries } = state;
  switch (e.type) {
    case "run_started":
      return state;
    case "text_start":
      return findIndex(entries, "reply", e.messageId) >= 0 ? ignore(state) : { ...state, entries: [...entries, { kind: "reply", id: e.messageId, text: "", end: "streaming" }] };
    case "text_delta": {
      const at = findIndex(entries, "reply", e.messageId);
      // Text for a message that never started still counts: it's the reply, out of order.
      if (at < 0) return { ...state, entries: [...entries, { kind: "reply", id: e.messageId, text: e.delta, end: "streaming" }] };
      const reply = entries[at];
      if (reply?.kind !== "reply" || reply.end !== "streaming") return ignore(state);
      return { ...state, entries: replace(entries, at, { ...reply, text: reply.text + e.delta }) };
    }
    case "text_end": {
      const at = findIndex(entries, "reply", e.messageId);
      const reply = entries[at];
      if (reply?.kind !== "reply" || reply.end !== "streaming") return ignore(state);
      return { ...state, entries: replace(entries, at, { ...reply, end: "done" }) };
    }
    case "tool_start":
      return findIndex(entries, "step", e.toolCallId) >= 0 ? ignore(state) : { ...state, entries: [...entries, { kind: "step", id: e.toolCallId, name: e.name, args: "", state: "running", answer: null }] };
    case "tool_args": {
      const at = findIndex(entries, "step", e.toolCallId);
      if (at < 0) return { ...state, entries: [...entries, { kind: "step", id: e.toolCallId, name: "tool", args: e.delta, state: "running", answer: null }] };
      const step = entries[at];
      if (step?.kind !== "step" || step.answer !== null) return ignore(state);
      return { ...state, entries: replace(entries, at, { ...step, args: step.args + e.delta }) };
    }
    case "tool_end":
      return findIndex(entries, "step", e.toolCallId) >= 0 ? state : ignore(state);
    case "tool_result": {
      const at = findIndex(entries, "step", e.toolCallId);
      const stepState = e.answer.kind === "error" ? "failed" : "done";
      if (at < 0) return { ...state, entries: [...entries, { kind: "step", id: e.toolCallId, name: "tool", args: "", state: stepState, answer: e.answer }] };
      const step = entries[at];
      if (step?.kind !== "step" || step.answer !== null) return ignore(state);
      return { ...state, entries: replace(entries, at, { ...step, state: stepState, answer: e.answer }) };
    }
    case "run_finished":
      if (e.stop === null) return finish(state, "done", null, "");
      return finish(state, endOfStop(e.stop), e.stop === "cancelled" ? "stopped" : "info", STOP_TEXT[e.stop] ?? `The run stopped early: ${e.stop}.`);
    case "run_error":
      // The client makes up an `abort` error when this side stops the run, so that's a stop, not a failure.
      if (e.code === "abort" || stopping) return finish(state, "stopped", "stopped", "Stopped.");
      return finish(state, "failed", "error", e.code === null ? e.message : `${e.message} (${e.code})`);
    case "unknown":
      return ignore(state);
  }
};

/** A thread as the server kept it: each tool call becomes a step with its answer, and a reply keeps why it ended early. */
export const transcriptOf = (messages: readonly ChatStoredMessage[]): readonly TranscriptEntry[] => {
  let entries: readonly TranscriptEntry[] = [];
  for (const m of [...messages].sort((a, b) => a.seq - b.seq)) {
    if (m.role === "user") {
      entries = [...entries, { kind: "user", id: m.id, text: m.text }];
    } else if (m.role === "tool") {
      const callId = m.toolCall?.id ?? m.id;
      const at = findIndex(entries, "step", callId);
      const answer = m.answer ?? { kind: "other" as const };
      const state = answer.kind === "error" ? "failed" : "done";
      const step = entries[at];
      entries = step?.kind === "step" ? replace(entries, at, { ...step, state, answer }) : [...entries, { kind: "step", id: callId, name: m.toolCall?.name ?? "tool", args: m.toolCall?.args ?? "", state, answer }];
    } else {
      if (m.toolCall !== null && findIndex(entries, "step", m.toolCall.id) < 0) {
        entries = [...entries, { kind: "step", id: m.toolCall.id, name: m.toolCall.name, args: m.toolCall.args, state: "done", answer: null }];
      }
      if (m.text !== "" || m.toolCall === null) entries = [...entries, { kind: "reply", id: m.id, text: m.text, end: m.stop === null ? "done" : endOfStop(m.stop) }];
    }
  }
  return entries;
};

export const transcriptReducer = (state: TranscriptState, action: TranscriptAction): TranscriptState => {
  switch (action.type) {
    case "sent":
      if (state.status !== "idle") return ignore(state);
      return { ...state, status: "running", runId: action.runId, entries: [...state.entries, { kind: "user", id: action.messageId, text: action.text }] };
    case "stop_requested":
      return state.status === "running" ? { ...state, status: "stopping" } : state;
    case "channel":
      // The stream closing after its end event is the normal way a run ends, and says nothing new.
      if (action.runId === state.runId && state.status === "idle" && action.event.kind === "closed") return state;
      // Late news of an earlier run, or of this one after it ended.
      if (action.runId !== state.runId || state.status === "idle") return ignore(state);
      return applyEvent(state, action.event);
    case "loaded":
      return { ...EMPTY_TRANSCRIPT, entries: transcriptOf(action.messages) };
  }
};

// ---- Steps

const argsOf = (args: string): Readonly<Record<string, unknown>> => {
  try {
    const parsed: unknown = JSON.parse(args);
    return typeof parsed === "object" && parsed !== null && !Array.isArray(parsed) ? (parsed as Readonly<Record<string, unknown>>) : {};
  } catch {
    return {};
  }
};

const TOOL_VERBS: Readonly<Record<string, string>> = { search: "Searched", read: "Read", provenance: "Traced", propose: "Proposed a change" };

/** One line saying what a step did, from its name and arguments, which are the model's words and shown only as text. */
export const stepSummary = (step: Extract<TranscriptEntry, { kind: "step" }>): string => {
  const args = argsOf(step.args);
  const str = (key: string): string | null => (typeof args[key] === "string" ? args[key] : null);
  const running = step.state === "running";
  switch (step.name) {
    case "search": {
      const q = str("q");
      const found = step.answer?.kind === "search" ? ` · ${String(step.answer.items.length)} found` : "";
      return `${running ? "Searching" : "Searched"} for “${q ?? "…"}”${found}`;
    }
    case "read":
      return `${running ? "Reading" : "Read"} ${str("kind") ?? "an object"} ${str("id") ?? ""}`.trim();
    case "provenance":
      return `${running ? "Tracing" : "Traced"} where ${str("id") ?? "a claim"} came from`;
    case "propose":
      return running ? "Proposing a change" : "Proposed a change";
    default:
      return `${TOOL_VERBS[step.name] ?? `Called ${step.name}`}`;
  }
};

// ---- Citations

/** Where a citation id leads in the console, from what this thread's tools returned. An id nothing returned leads nowhere. */
export type CitationIndex = ReadonlyMap<string, string>;

const segmentRoute = (sourceId: string, over: Partial<SourceViewRoute>): SourceViewRoute => ({ kind: "source", id: sourceId, tab: "view", page: null, zoom: null, claim: null, evidence: null, fromMs: null, toMs: null, rendition: null, segment: null, ...over });

const addItem = (index: Map<string, string>, vault: VaultId, item: EvidenceItem) => {
  for (const citation of item.citations) {
    const path = citationPath(vault, item, citation);
    if (path === null) continue;
    if (!index.has(item.id)) index.set(item.id, path);
    // A segment named in a claim's citation still opens as itself.
    if (citation.segmentId !== null && !index.has(citation.segmentId) && citation.sourceId !== null) {
      const page = citation.rects[0]?.page ?? citation.pageFrom;
      const time = citation.tStartMs === null ? {} : { fromMs: citation.tStartMs, toMs: citation.tEndMs };
      index.set(citation.segmentId, documentPath(vault, segmentRoute(citation.sourceId, { page, segment: citation.segmentId, ...time })));
    }
  }
  if (!index.has(item.id)) {
    if (item.kind === "entity") index.set(item.id, sectionPath(vault, "entities", item.id));
    else if ((item.kind === "page" || item.kind === "block") && item.pageId !== null) index.set(item.id, sectionPath(vault, "pages", item.pageId));
  }
};

const addAnswer = (index: Map<string, string>, vault: VaultId, answer: ChatToolAnswer) => {
  switch (answer.kind) {
    case "search":
      for (const item of answer.items) addItem(index, vault, item);
      return;
    case "segment": {
      const s = answer.segment;
      if (index.has(s.id)) return;
      const time = s.tStartMs === null ? {} : { fromMs: s.tStartMs, toMs: s.tEndMs };
      index.set(s.id, documentPath(vault, segmentRoute(s.sourceId, { page: s.pageFrom, segment: s.id, ...time })));
      return;
    }
    case "claim": {
      const l = answer.lineage;
      const first = l.evidence[0];
      if (l.claimId !== "" && !index.has(l.claimId)) {
        index.set(l.claimId, first === undefined ? documentPath(vault, { kind: "lineage", claim: l.claimId, evidence: null }) : documentPath(vault, segmentRoute(first.sourceId, { page: first.rects[0]?.page ?? null, claim: l.claimId, evidence: first.evidenceId, ...(first.tStartMs === null ? {} : { fromMs: first.tStartMs, toMs: first.tEndMs }) })));
      }
      for (const e of l.evidence) {
        if (!index.has(e.segmentId)) index.set(e.segmentId, documentPath(vault, segmentRoute(e.sourceId, { page: e.rects[0]?.page ?? null, segment: e.segmentId, ...(e.tStartMs === null ? {} : { fromMs: e.tStartMs, toMs: e.tEndMs }) })));
      }
      return;
    }
    case "object":
      if (index.has(answer.id)) return;
      if (answer.objectKind === "page") index.set(answer.id, sectionPath(vault, "pages", answer.id));
      else if (answer.objectKind === "entity") index.set(answer.id, sectionPath(vault, "entities", answer.id));
      else if (answer.objectKind === "source") index.set(answer.id, documentPath(vault, segmentRoute(answer.id, {})));
      return;
    case "proposal":
    case "error":
    case "other":
      return;
  }
};

export const citationIndex = (vault: VaultId, entries: readonly TranscriptEntry[]): CitationIndex => {
  const index = new Map<string, string>();
  for (const e of entries) if (e.kind === "step" && e.answer !== null) addAnswer(index, vault, e.answer);
  return index;
};

export type ReplyPart = { readonly kind: "text"; readonly text: string } | { readonly kind: "cite"; readonly id: string; readonly path: string | null };

const CITE = /\[cite:\s*([^\]\s]{1,200})\s*\]/g;

/** A reply split at its `[cite:ID]` marks. A mark still being streamed stays text until its `]` arrives. */
export const replyParts = (text: string, index: CitationIndex): readonly ReplyPart[] => {
  const parts: ReplyPart[] = [];
  let last = 0;
  for (const match of text.matchAll(CITE)) {
    const id = match[1] ?? "";
    if (match.index > last) parts.push({ kind: "text", text: text.slice(last, match.index) });
    parts.push({ kind: "cite", id, path: index.get(id) ?? null });
    last = match.index + match[0].length;
  }
  if (last < text.length) parts.push({ kind: "text", text: text.slice(last) });
  return parts;
};

export interface PreparedReply {
  /** Markdown, with each citation as an `INLINE_OPEN<n>INLINE_CLOSE` placeholder for `SafeMarkdown`. */
  readonly text: string;
  readonly cites: readonly Extract<ReplyPart, { kind: "cite" }>[];
}

/** A reply ready for `SafeMarkdown`. Placeholder characters already in the reply are replaced first, so the model can't forge a chip. */
export const prepareReply = (text: string, index: CitationIndex): PreparedReply => {
  const clean = text.replaceAll(INLINE_OPEN, "\uFFFD").replaceAll(INLINE_CLOSE, "\uFFFD");
  const cites: Extract<ReplyPart, { kind: "cite" }>[] = [];
  const out = replyParts(clean, index)
    .map((part) => {
      if (part.kind === "text") return part.text;
      cites.push(part);
      return `${INLINE_OPEN}${String(cites.length - 1)}${INLINE_CLOSE}`;
    })
    .join("");
  return { text: out, cites };
};

/** How many of a reply's citations lead nowhere, which the reply says. */
export const unresolvedCount = (parts: readonly ReplyPart[]): number => parts.filter((p) => p.kind === "cite" && p.path === null).length;

// ---- What a person can attach

export interface ChatAttachment {
  readonly kind: "source" | "page" | "entity" | "review item";
  readonly id: string;
}

/** The object the current screen shows, if there's one to offer. It's only offered: nothing reaches the chat unless the person adds it. */
export const attachableFrom = (section: string | null, rest: readonly string[]): ChatAttachment | null => {
  const [first, second] = rest;
  if (first === undefined || first === "" || first === "-") return null;
  if (section === "documents" && second === undefined) return { kind: "source", id: first };
  if (section === "pages" && second === undefined) return { kind: "page", id: first };
  if (section === "entities" && second === undefined) return { kind: "entity", id: first };
  if (section === "review" && second === undefined) return { kind: "review item", id: first };
  return null;
};

/** The message as sent. The attachment is plain text the agent reads, naming the object so it can look it up with its tools. */
export const messageWith = (text: string, attachment: ChatAttachment | null): string =>
  attachment === null ? text : `${text}\n\n(I'm looking at the ${attachment.kind} with id ${attachment.id}.)`;

export const MAX_MESSAGE_CHARS = 8_000;
