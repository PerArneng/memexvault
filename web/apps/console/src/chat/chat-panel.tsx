import { citationIndex, MAX_MESSAGE_CHARS, messageWith, scopeIn } from "@memex/core";
import type { ChatAttachment, ChatSession } from "@memex/core";
import type { ChatSettings, Identity, VaultId } from "@memex/types";
import { Button, EmptyState, Skeleton } from "@memex/ui";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { MessageSquarePlus, Send, Square, Trash2, X } from "lucide-react";
import { useEffect, useId, useMemo, useRef, useState, useSyncExternalStore } from "react";
import type { KeyboardEvent } from "react";
import { useEngine } from "../engine-context";
import { QUERY_KEYS, unwrap } from "../query";
import { QueryView } from "../shell/query-view";
import { TranscriptView } from "./transcript-view";

const tokens = (n: number) => n.toLocaleString("en");

/** What answers, where, and what's left to spend. Only an admin is told the endpoint, so it shows only when the server sent it. */
const Banner = ({ settings }: { readonly settings: ChatSettings }) => {
  const left = Math.max(0, settings.dayTokensLimit - settings.dayTokensUsed);
  const low = settings.dayTokensLimit > 0 && left < settings.dayTokensLimit / 10;
  return (
    <div data-testid="chat-banner" className="flex flex-col gap-0.5 border-b border-border px-3 py-2 text-xs text-text-muted">
      <span>
        Answered by <span data-testid="chat-model" className="font-mono text-text">{settings.model}</span>
        {settings.endpoint !== null && <> at <span data-testid="chat-endpoint" className="font-mono text-text">{settings.endpoint}</span></>}. Your messages and what it reads in this vault go to that model.
      </span>
      <span data-testid="chat-budget" className={low ? "text-warning" : ""}>
        Today: {tokens(settings.dayTokensUsed)} of {tokens(settings.dayTokensLimit)} tokens used{settings.threadTokenBudget !== null ? ` · ${tokens(settings.threadTokenBudget)} per thread` : ""}
        {left === 0 && ". Today's budget is spent, so nothing more can be answered until midnight UTC."}
      </span>
    </div>
  );
};

const useTranscript = (session: ChatSession) => useSyncExternalStore(session.subscribe, session.state);

interface ConversationProps {
  readonly identity: Identity;
  readonly vault: VaultId;
  readonly threadId: string | null;
  readonly attachable: ChatAttachment | null;
  readonly onThreadUsed: (id: string) => void;
}

const Conversation = ({ identity, vault, threadId, attachable, onThreadUsed }: ConversationProps) => {
  const engine = useEngine();
  const queryClient = useQueryClient();
  const session = useMemo(() => engine.openChat(vault, threadId), [engine, vault, threadId]);
  useEffect(() => () => session.close(), [session]);
  const state = useTranscript(session);
  const history = useQuery({
    queryKey: QUERY_KEYS.chatThread(vault, threadId ?? ""),
    queryFn: async () => unwrap(await engine.getChatThread(vault, threadId ?? "")),
    enabled: threadId !== null,
    meta: { ownsErrors: true },
    staleTime: Infinity,
  });
  useEffect(() => {
    if (history.data !== undefined && session.state().entries.length === 0) session.load(history.data);
  }, [history.data, session]);

  const [draft, setDraft] = useState("");
  const [attach, setAttach] = useState(false);
  const inputId = useId();
  const running = state.status !== "idle";
  const wasRunning = useRef(false);
  useEffect(() => {
    // A run that ended has changed the thread list and today's spending.
    if (wasRunning.current && !running) void queryClient.invalidateQueries({ queryKey: QUERY_KEYS.chat(vault) });
    wasRunning.current = running;
  }, [running, queryClient, vault]);

  const index = useMemo(() => citationIndex(vault, state.entries), [vault, state.entries]);
  const bottom = useRef<HTMLDivElement>(null);
  useEffect(() => {
    bottom.current?.scrollIntoView({ block: "end" });
  }, [state.entries]);

  const send = () => {
    const text = draft.trim();
    if (text === "" || running) return;
    session.send(messageWith(text, attach ? attachable : null));
    onThreadUsed(session.threadId);
    setDraft("");
    setAttach(false);
  };
  const onKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (e.key === "Enter" && !e.shiftKey) {
      e.preventDefault();
      send();
    }
  };

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      <div className="min-h-0 flex-1 overflow-auto px-3 py-3" data-testid="chat-transcript" data-run-status={state.status}>
        {threadId !== null && history.isPending ? (
          <Skeleton className="h-16 w-full" />
        ) : history.isError ? (
          <p role="alert" className="text-sm text-danger">This thread couldn't be read.</p>
        ) : state.entries.length === 0 ? (
          <p className="text-sm text-text-muted">Ask about anything in this vault. Answers cite what they rest on, and a citation opens its source.</p>
        ) : (
          <TranscriptView identity={identity} vault={vault} entries={state.entries} index={index} />
        )}
        <div ref={bottom} />
      </div>
      <div className="flex flex-col gap-2 border-t border-border p-3">
        {attachable !== null && (
          <label className="flex items-center gap-2 text-xs text-text-muted">
            <input type="checkbox" checked={attach} onChange={(e) => setAttach(e.target.checked)} data-testid="chat-attach" />
            Add the {attachable.kind} on this screen to my message
          </label>
        )}
        <label htmlFor={inputId} className="sr-only">Message</label>
        <textarea
          id={inputId}
          rows={3}
          maxLength={MAX_MESSAGE_CHARS}
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={onKeyDown}
          placeholder="Ask this vault…"
          className="resize-none rounded-md border border-border-strong bg-surface px-2 py-1 text-sm text-text"
        />
        <div className="flex items-center gap-2">
          {running ? (
            <Button variant="danger" onClick={() => session.stop()} disabled={state.status === "stopping"}>
              <Square size={14} aria-hidden={true} /> Stop
            </Button>
          ) : (
            <Button variant="primary" onClick={send} disabled={draft.trim() === ""}>
              <Send size={14} aria-hidden={true} /> Send
            </Button>
          )}
          <span className="text-xs text-text-muted">Enter to send, Shift+Enter for a new line</span>
        </div>
      </div>
    </div>
  );
};

const Threads = ({ vault, current, onOpen }: { readonly vault: VaultId; readonly current: string | null; readonly onOpen: (id: string | null) => void }) => {
  const engine = useEngine();
  const queryClient = useQueryClient();
  const threads = useQuery({ queryKey: QUERY_KEYS.chatThreads(vault), queryFn: async () => unwrap(await engine.listChatThreads(vault, null)) });
  const remove = async (id: string) => {
    await engine.deleteChatThread(vault, id);
    if (id === current) onOpen(null);
    await queryClient.invalidateQueries({ queryKey: QUERY_KEYS.chat(vault) });
  };
  return (
    <nav aria-label="Chat threads" className="flex max-h-40 flex-col gap-1 overflow-auto border-b border-border px-3 py-2">
      <Button variant="ghost" className="justify-start" onClick={() => onOpen(null)}>
        <MessageSquarePlus size={14} aria-hidden={true} /> New thread
      </Button>
      {threads.data?.items.map((t) => (
        <div key={t.id} className="flex items-center gap-1">
          <button type="button" onClick={() => onOpen(t.id)} aria-current={t.id === current ? "true" : undefined} data-thread={t.id} className={`min-w-0 flex-1 truncate rounded-md px-2 py-1 text-left text-sm ${t.id === current ? "bg-surface-selected text-text" : "text-text-muted hover:bg-surface-overlay"}`}>
            {t.title}
          </button>
          <button type="button" aria-label={`Delete the thread ${t.title}`} onClick={() => void remove(t.id)} className="rounded-md p-1 text-text-muted hover:text-danger">
            <Trash2 size={13} aria-hidden={true} />
          </button>
        </div>
      ))}
    </nav>
  );
};

export interface ChatPanelProps {
  readonly identity: Identity;
  readonly vault: VaultId;
  /** The object the screen shows. It reaches the chat only if the person ticks the box. */
  readonly attachable: ChatAttachment | null;
  readonly onClose?: () => void;
}

/** The chat beside any screen. Lazy, with the AG-UI client loaded on the first message. */
const ChatPanel = ({ identity, vault, attachable, onClose }: ChatPanelProps) => {
  const engine = useEngine();
  const settings = useQuery({ queryKey: QUERY_KEYS.chatSettings(vault), queryFn: async () => unwrap(await engine.getChatSettings(vault)) });
  const [threadId, setThreadId] = useState<string | null>(null);
  // A new thread gets its id from the session, and keeps it once something is sent.
  const [fresh, setFresh] = useState(0);
  // The thread being talked in, which a new one becomes once something is sent.
  const [shown, setShown] = useState<string | null>(null);
  const isAdmin = scopeIn(identity, vault) === "admin";
  return (
    <section aria-label="Chat panel" data-testid="chat-panel" className="flex h-full min-h-0 flex-col bg-surface-raised">
      <header className="flex items-center gap-2 border-b border-border px-3 py-2">
        <h2 className="text-sm font-medium">Chat</h2>
        {onClose !== undefined && (
          <Button variant="ghost" className="ml-auto" aria-label="Close the chat" onClick={onClose}>
            <X size={14} aria-hidden={true} />
          </Button>
        )}
      </header>
      <QueryView query={settings} label="this vault's chat">
        {(s) =>
          s.model === null ? (
            <div className="p-3" data-testid="chat-off" data-chat-off="no_model">
              <EmptyState title="This server has no chat model.">An operator sets one with MEMEX_CHAT_MODEL. Until then nothing in any vault can be asked.</EmptyState>
            </div>
          ) : !s.enabled ? (
            <div className="p-3" data-testid="chat-off" data-chat-off="vault">
              <EmptyState title="Chat is off for this vault.">
                {isAdmin ? "Turning it on sends this vault's content to the configured model. An admin turns it on through the API, as the settings screen isn't built yet." : "An admin of this vault can turn it on."}
              </EmptyState>
            </div>
          ) : (
            <>
              <Banner settings={s} />
              <Threads vault={vault} current={shown} onOpen={(id) => { setThreadId(id); setShown(id); setFresh((n) => n + 1); }} />
              <Conversation key={`${threadId ?? "new"}-${String(fresh)}`} identity={identity} vault={vault} threadId={threadId} attachable={attachable} onThreadUsed={setShown} />
            </>
          )
        }
      </QueryView>
    </section>
  );
};

export default ChatPanel;
