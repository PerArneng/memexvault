import type { ChannelEvent, ChatSettings, ChatThread, ChatThreadDetail, Page, VaultId } from "@memex/types";
import { failed, ok } from "./api";
import type { ApiResult } from "./api";
import { EMPTY_TRANSCRIPT, transcriptReducer } from "./chat";
import type { AgentChannel, AgentRun, TranscriptAction, TranscriptState } from "./chat";
import type { IdSource } from "./wiki-cases";

/** The chat API, apart from the run itself, which goes through an `AgentChannel`. */
export interface ChatApi {
  getChatSettings(vault: VaultId): Promise<ApiResult<ChatSettings>>;
  listChatThreads(vault: VaultId, cursor: string | null): Promise<ApiResult<Page<ChatThread>>>;
  getChatThread(vault: VaultId, id: string, cursor: string | null): Promise<ApiResult<ChatThreadDetail>>;
  deleteChatThread(vault: VaultId, id: string): Promise<ApiResult<null>>;
}

/** One thread as it's being talked in. Its state changes only through the transcript reducer. */
export interface ChatSession {
  readonly threadId: string;
  state(): TranscriptState;
  /** Called on every change. Returns the way to stop. */
  subscribe(listener: () => void): () => void;
  /** Nothing happens while a run is going. */
  send(text: string): void;
  stop(): void;
  /** Replaces the transcript with what the server kept. */
  load(detail: ChatThreadDetail): void;
  /** Stops any run, and listens to nothing more. */
  close(): void;
}

export interface ChatCases {
  getChatSettings(vault: VaultId): Promise<ApiResult<ChatSettings>>;
  listChatThreads(vault: VaultId, cursor: string | null): Promise<ApiResult<Page<ChatThread>>>;
  /** The whole thread, page by page, up to a bound. */
  getChatThread(vault: VaultId, id: string): Promise<ApiResult<ChatThreadDetail>>;
  deleteChatThread(vault: VaultId, id: string): Promise<ApiResult<null>>;
  /** A thread to talk in: the one named, or a new one. */
  openChat(vault: VaultId, threadId: string | null): ChatSession;
}

interface ChatDeps {
  readonly edges: {
    readonly api: ChatApi;
    readonly ids: IdSource;
    /** Loads the channel, which brings in the AG-UI client, only when someone first sends. Left out, chat says it can't run here. */
    readonly agentChannel?: () => Promise<AgentChannel>;
  };
  readonly watched: <T>(call: Promise<ApiResult<T>>) => Promise<ApiResult<T>>;
}

/** 20 pages of 200 is 4,000 messages, which no budget a thread can have would reach. */
const MAX_THREAD_PAGES = 20;

export const createChatSession = (vault: VaultId, threadId: string, ids: IdSource, channel: (() => Promise<AgentChannel>) | undefined): ChatSession => {
  let state = EMPTY_TRANSCRIPT;
  let current: AgentRun | null = null;
  let closed = false;
  const listeners = new Set<() => void>();
  const dispatch = (action: TranscriptAction) => {
    const next = transcriptReducer(state, action);
    if (next === state) return;
    state = next;
    for (const listener of [...listeners]) listener();
  };
  const tell = (runId: string) => (event: ChannelEvent) => {
    if (event.kind === "closed") current = null;
    dispatch({ type: "channel", runId, event });
  };
  return {
    threadId,
    state: () => state,
    subscribe: (listener) => {
      listeners.add(listener);
      return () => listeners.delete(listener);
    },
    send: (text) => {
      if (closed || state.status !== "idle" || text.trim() === "") return;
      const runId = ids.next();
      const messageId = ids.next();
      dispatch({ type: "sent", runId, messageId, text });
      const listener = tell(runId);
      if (channel === undefined) {
        listener({ kind: "refused", status: 0, code: null, message: "chat can't run in this console" });
        listener({ kind: "closed" });
        return;
      }
      void channel().then(
        (c) => {
          // Stopped, or closed, while the channel was still loading: nothing goes to the server.
          if (closed || state.runId !== runId || state.status !== "running") {
            listener({ kind: "closed" });
            return;
          }
          current = c.run({ vault, threadId, runId, messageId, message: text }, listener);
        },
        () => {
          listener({ kind: "dropped", message: "the chat client couldn't be loaded" });
          listener({ kind: "closed" });
        },
      );
    },
    stop: () => {
      if (state.status !== "running") return;
      dispatch({ type: "stop_requested" });
      const run = current;
      if (run === null) {
        // Nothing has gone out yet.
        const runId = state.runId ?? "";
        dispatch({ type: "channel", runId, event: { kind: "closed" } });
        return;
      }
      run.stop();
    },
    load: (detail) => {
      if (state.status !== "idle") return;
      dispatch({ type: "loaded", messages: detail.messages });
    },
    close: () => {
      closed = true;
      current?.stop();
      current = null;
      listeners.clear();
    },
  };
};

export const createChatCases = ({ edges, watched }: ChatDeps): ChatCases => {
  const { api, ids, agentChannel } = edges;
  return {
    getChatSettings: (vault) => watched(api.getChatSettings(vault)),
    listChatThreads: (vault, cursor) => watched(api.listChatThreads(vault, cursor)),
    getChatThread: async (vault, id) => {
      let cursor: string | null = null;
      let first: ChatThreadDetail | null = null;
      const messages: ChatThreadDetail["messages"][number][] = [];
      for (let page = 0; page < MAX_THREAD_PAGES; page += 1) {
        const result: ApiResult<ChatThreadDetail> = await watched(api.getChatThread(vault, id, cursor));
        if (!result.ok) return failed(result.failure);
        first ??= result.value;
        messages.push(...result.value.messages);
        cursor = result.value.nextCursor;
        if (cursor === null) break;
      }
      if (first === null) return failed({ kind: "unreachable", message: "no answer" });
      return ok({ thread: first.thread, messages, nextCursor: cursor });
    },
    deleteChatThread: (vault, id) => watched(api.deleteChatThread(vault, id)),
    openChat: (vault, threadId) => createChatSession(vault, threadId ?? ids.next(), ids, agentChannel),
  };
};
