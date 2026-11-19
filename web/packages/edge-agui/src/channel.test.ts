// Spec 050: the real HttpAgent, on every stream spec 049's backend produced, through a fetch that answers from those files.
import { readdirSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import type { ChannelEvent } from "@memex/types";
import { vaultId } from "@memex/types";
import { describe, expect, it } from "vitest";
import { createAgUiAgentChannel } from "./index";

const folder = join(dirname(fileURLToPath(import.meta.url)), "../../../../crates/memex-http/tests/fixtures/agui");
const files = readdirSync(folder).filter((f) => f.endsWith(".jsonl"));
const sse = (file: string) =>
  readFileSync(join(folder, file), "utf8")
    .split("\n")
    .filter(Boolean)
    .map((line) => `data: ${line}\n\n`)
    .join("");

const request = { vault: vaultId("v1"), threadId: "thread-1", runId: "run-1", messageId: "m-1", message: "How long is the bridge?" };

interface Sent {
  url: string;
  init: RequestInit;
}

/** Runs one request and collects what the listener heard until `closed`. */
const collect = (fetch: (url: string, init: RequestInit) => Promise<Response>, during?: (stop: () => void) => void) =>
  new Promise<ChannelEvent[]>((done) => {
    const heard: ChannelEvent[] = [];
    const run = createAgUiAgentChannel({ apiBaseUrl: "/api/v1", fetch }).run(request, (e) => {
      heard.push(e);
      if (e.kind === "closed") done(heard);
    });
    during?.(() => run.stop());
  });

const answering = (body: BodyInit, status = 200, contentType = "text/event-stream") => {
  const sent: Sent[] = [];
  const fetch = (url: string, init: RequestInit) => {
    sent.push({ url, init });
    return Promise.resolve(new Response(body, { status, headers: { "content-type": contentType } }));
  };
  return { sent, fetch };
};

describe("AgUiAgentChannel", () => {
  it("reads every backend stream to exactly one end, with no event it doesn't know", async () => {
    expect(files.length).toBeGreaterThan(20);
    for (const file of files) {
      const { fetch } = answering(sse(file));
      const heard = await collect(fetch);
      const events = heard.flatMap((e) => (e.kind === "event" ? [e.event] : []));
      expect(heard.at(-1), file).toEqual({ kind: "closed" });
      expect(heard.filter((e) => e.kind === "closed"), file).toHaveLength(1);
      expect(events.filter((e) => e.type === "unknown"), file).toEqual([]);
      expect(events.filter((e) => e.type === "run_finished" || e.type === "run_error"), file).toHaveLength(1);
    }
  });

  it("sends only the newest message, with the CSRF header, to the vault's run route", async () => {
    const { sent, fetch } = answering(sse("text.jsonl"));
    await collect(fetch);
    expect(sent).toHaveLength(1);
    const [one] = sent;
    expect(one?.url).toBe("/api/v1/vaults/v1/chat/runs");
    expect((one?.init.headers as Record<string, string>)["x-memex-csrf"]).toBe("1");
    const body = JSON.parse(String(one?.init.body)) as { threadId: string; runId: string; messages: { role: string; content: string }[] };
    expect(body).toMatchObject({ threadId: "thread-1", runId: "run-1" });
    expect(body.messages).toEqual([{ id: "m-1", role: "user", content: "How long is the bridge?" }]);
  });

  it("reads a refusal's status and code from the server's error body", async () => {
    const { fetch } = answering(JSON.stringify({ error: { code: "chat_disabled", message: "chat is off for this vault", request_id: "r" } }), 403, "application/json");
    expect(await collect(fetch)).toEqual([{ kind: "refused", status: 403, code: "chat_disabled", message: "chat is off for this vault" }, { kind: "closed" }]);
  });

  it("says a broken connection dropped, after what arrived", async () => {
    const encoder = new TextEncoder();
    let pulls = 0;
    const stream = new ReadableStream<Uint8Array>({
      pull: (c) => {
        pulls += 1;
        if (pulls === 1) c.enqueue(encoder.encode('data: {"type":"RUN_STARTED","runId":"run-1","threadId":"thread-1"}\n\ndata: {"type":"TEXT_MESSAGE_START","messageId":"m","role":"assistant"}\n\n'));
        else c.error(new TypeError("network error"));
      },
    });
    const heard = await collect(answering(stream).fetch);
    expect(heard.map((e) => e.kind)).toEqual(["event", "event", "dropped", "closed"]);
  });

  it("stops at once mid-stream, says so as an abort, and aborts the request", async () => {
    const stream = new ReadableStream<Uint8Array>({
      start: (c) => c.enqueue(new TextEncoder().encode('data: {"type":"RUN_STARTED","runId":"run-1","threadId":"thread-1"}\n\ndata: {"type":"TEXT_MESSAGE_START","messageId":"m","role":"assistant"}\n\ndata: {"type":"TEXT_MESSAGE_CONTENT","messageId":"m","delta":"The bri"}\n\n'))
    });
    let signal: AbortSignal | null = null;
    const fetch = (_url: string, init: RequestInit) => {
      signal = init.signal ?? null;
      return Promise.resolve(new Response(stream, { headers: { "content-type": "text/event-stream" } }));
    };
    const heard = await collect(fetch, (stop) => {
      // After the three frames have been read.
      void new Promise((r) => setTimeout(r, 20)).then(stop);
    });
    const events = heard.flatMap((e) => (e.kind === "event" ? [e.event] : []));
    expect(events.map((e) => e.type)).toEqual(["run_started", "text_start", "text_delta", "run_error"]);
    expect(events.at(-1)).toMatchObject({ code: "abort" });
    expect(heard.at(-1)).toEqual({ kind: "closed" });
    // A real fetch closes the connection on this signal, which the server takes as the cancel. The e2e checks measure that.
    expect((signal as AbortSignal | null)?.aborted).toBe(true);
  });

  it("stops before the server has answered at all", async () => {
    const fetch = (_url: string, init: RequestInit) =>
      new Promise<Response>((_resolve, reject) => {
        init.signal?.addEventListener("abort", () => reject(new DOMException("aborted", "AbortError")));
      });
    const heard = await collect(fetch, (stop) => stop());
    expect(heard).toEqual([{ kind: "event", event: { type: "run_error", message: "Request aborted", code: "abort" } }, { kind: "closed" }]);
  });
});
