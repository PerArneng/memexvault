import { epochMs, vaultId } from "@memex/types";
import type { ChatThreadDetail } from "@memex/types";
import { describe, expect, it } from "vitest";
import { createChatCases, createChatSession } from "./chat-cases";
import type { AgentChannel } from "./chat";
import { countingIds, fakeApi, scriptedAgentChannel } from "./testing";

const v = vaultId("v1");
const watched = <T>(call: Promise<T>) => call;
/** Lets settled promises run, with no clock. */
const flush = async () => {
  for (let i = 0; i < 10; i += 1) await Promise.resolve();
};

const thread: ChatThreadDetail = {
  thread: { id: "t1", title: "Bridge", createdAt: epochMs(1), updatedAt: epochMs(2), tokens: 9 },
  messages: [1, 2, 3, 4, 5].map((seq) => ({ seq, id: `m${String(seq)}`, runId: "r", role: seq % 2 === 1 ? ("user" as const) : ("assistant" as const), text: `say ${String(seq)}`, toolCall: null, answer: null, stop: null, at: epochMs(seq) })),
  nextCursor: null,
};

describe("a chat session", () => {
  it("sends the newest message on its own and follows the run to its end", async () => {
    const channel = scriptedAgentChannel();
    const session = createChatSession(v, "t1", countingIds(), () => Promise.resolve(channel));
    let changes = 0;
    session.subscribe(() => (changes += 1));
    session.send("How long is the bridge?");
    expect(session.state().status).toBe("running");
    await flush();
    const [run] = channel.runs;
    expect(run?.request).toEqual({ vault: v, threadId: "t1", runId: "id-1", messageId: "id-2", message: "How long is the bridge?" });
    run?.event({ type: "text_start", messageId: "a" });
    run?.event({ type: "text_delta", messageId: "a", delta: "412 m." });
    run?.event({ type: "run_finished", runId: "id-1", stop: null });
    run?.emit({ kind: "closed" });
    expect(session.state().status).toBe("idle");
    expect(session.state().entries.map((e) => e.kind)).toEqual(["user", "reply"]);
    expect(changes).toBeGreaterThanOrEqual(4);
  });

  it("stops a run, which the channel ends at once, and keeps the partial reply as stopped", async () => {
    const channel = scriptedAgentChannel();
    const session = createChatSession(v, "t1", countingIds(), () => Promise.resolve(channel));
    session.send("q");
    await flush();
    channel.runs[0]?.event({ type: "text_delta", messageId: "a", delta: "The bri" });
    session.stop();
    expect(channel.runs[0]?.stopped).toBe(true);
    const s = session.state();
    expect(s.status).toBe("idle");
    expect(s.entries.find((e) => e.kind === "reply")).toMatchObject({ text: "The bri", end: "stopped" });
    // Anything the run still says after that is late.
    channel.runs[0]?.event({ type: "text_delta", messageId: "a", delta: "dge" });
    expect(session.state().entries.find((e) => e.kind === "reply")).toMatchObject({ text: "The bri" });
  });

  it("sends nothing when stopped while the channel is still loading", async () => {
    const channel = scriptedAgentChannel();
    let release: (c: AgentChannel) => void = () => undefined;
    const session = createChatSession(v, "t1", countingIds(), () => new Promise((done) => (release = done)));
    session.send("q");
    session.stop();
    expect(session.state().status).toBe("idle");
    release(channel);
    await flush();
    expect(channel.runs).toHaveLength(0);
  });

  it("says when the channel can't load, and when the console has none", async () => {
    const broken = createChatSession(v, "t1", countingIds(), () => Promise.reject(new Error("chunk failed")));
    broken.send("q");
    await flush();
    expect(broken.state().status).toBe("idle");
    expect(broken.state().entries.at(-1)).toMatchObject({ kind: "notice", tone: "error" });
    const none = createChatSession(v, "t1", countingIds(), undefined);
    none.send("q");
    expect(none.state().status).toBe("idle");
    expect(none.state().entries.at(-1)).toMatchObject({ kind: "notice", tone: "error" });
  });

  it("ignores an empty message and a send during a run, and stops listening once closed", async () => {
    const channel = scriptedAgentChannel();
    const session = createChatSession(v, "t1", countingIds(), () => Promise.resolve(channel));
    session.send("   ");
    expect(session.state().entries).toEqual([]);
    session.send("one");
    session.send("two");
    await flush();
    expect(channel.runs).toHaveLength(1);
    session.close();
    expect(channel.runs[0]?.stopped).toBe(true);
  });

  it("loads a thread's history only between runs", () => {
    const session = createChatSession(v, "t1", countingIds(), undefined);
    session.load(thread);
    expect(session.state().entries).toHaveLength(5);
  });
});

describe("chat cases", () => {
  it("reads a whole thread page by page", async () => {
    const api = fakeApi({ chat: { threads: [thread] } });
    const cases = createChatCases({ edges: { api, ids: countingIds() }, watched });
    const got = await cases.getChatThread(v, "t1");
    expect(got.ok && got.value.messages.map((m) => m.seq)).toEqual([1, 2, 3, 4, 5]);
    expect(api.calls.filter((c) => c.startsWith("getChatThread"))).toEqual(["getChatThread:t1:", "getChatThread:t1:2", "getChatThread:t1:4"]);
    const missing = await cases.getChatThread(v, "nope");
    expect(missing.ok).toBe(false);
  });

  it("opens a new thread with a fresh id, or the one named", () => {
    const cases = createChatCases({ edges: { api: fakeApi(), ids: countingIds() }, watched });
    expect(cases.openChat(v, null).threadId).toBe("id-1");
    expect(cases.openChat(v, "t7").threadId).toBe("t7");
  });
});
