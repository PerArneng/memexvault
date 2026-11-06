// Shared by verify.mjs and overhead.mjs. Uses HttpAgent, so @ag-ui/client's own ordering rules apply to every run.
import { HttpAgent } from "@ag-ui/client";
import { EventSchemas, RunAgentInputSchema } from "@ag-ui/core/schemas";

export const BASE = process.env.BASE ?? "http://127.0.0.1:18034";
let n = 0;
export const newRunId = () => `run-${Date.now()}-${++n}`;

export async function stats(runId) {
  const r = await fetch(`${BASE}/runs/${runId}`);
  return r.ok ? r.json() : null;
}

/** Runs one chat through HttpAgent. Every event is validated against @ag-ui/core. */
export async function run({ provider, mode = "chat", prompt, maxOutputTokens = 400, abortAfterDeltas, abortAfterMs }) {
  const runId = newRunId();
  const threadId = `thread-${runId}`;
  const messages = [{ id: `${runId}-u`, role: "user", content: prompt }];
  RunAgentInputSchema.parse({ threadId, runId, messages, tools: [], context: [], state: {}, forwardedProps: {} });
  const agent = new HttpAgent({ url: `${BASE}/agent/${provider}/${mode}`, threadId, initialMessages: messages });
  const res = { runId, events: [], types: [], invalid: [], deltas: [], t0: performance.now(), firstDeltaMs: null, abortWall: null, thrown: null };
  const abort = () => { if (res.abortWall === null) { res.abortWall = Date.now(); agent.abortRun(); } };
  if (abortAfterMs) setTimeout(abort, abortAfterMs);
  try {
    await agent.runAgent({ runId, forwardedProps: { maxOutputTokens } }, {
      onEvent({ event }) {
        res.events.push(event);
        res.types.push(event.type);
        const parsed = EventSchemas.safeParse(event);
        if (!parsed.success) res.invalid.push({ type: event.type, issue: parsed.error.message });
        if (event.type === "TEXT_MESSAGE_CONTENT") {
          res.firstDeltaMs ??= performance.now() - res.t0;
          res.deltas.push(event.delta);
          if (abortAfterDeltas && res.deltas.length === abortAfterDeltas) abort();
        }
      },
    });
  } catch (e) {
    res.thrown = String(e?.message ?? e);
  }
  res.totalMs = performance.now() - res.t0;
  res.assistantText = agent.messages.filter((m) => m.role === "assistant").map((m) => m.content ?? "").join("");
  res.toolMessages = agent.messages.filter((m) => m.role === "tool");
  res.server = await stats(runId);
  return res;
}

/** The same run by plain fetch, to see the wire: raw frames and whether the body ends cleanly. */
export async function rawRun({ provider, mode = "chat", prompt = "Hi" }) {
  const runId = newRunId();
  const r = await fetch(`${BASE}/agent/${provider}/${mode}`, {
    method: "POST",
    headers: { "Content-Type": "application/json", Accept: "text/event-stream" },
    body: JSON.stringify({ threadId: `thread-${runId}`, runId, messages: [{ id: "u", role: "user", content: prompt }], tools: [], context: [], state: {}, forwardedProps: {} }),
  });
  let text = "", cleanEnd = true;
  try {
    const decoder = new TextDecoder();
    for await (const chunk of r.body) text += decoder.decode(chunk, { stream: true });
  } catch { cleanEnd = false; }
  const frames = text.split("\n\n").filter((f) => f.startsWith("data:")).map((f) => JSON.parse(f.slice(5)));
  return { status: r.status, contentType: r.headers.get("content-type"), cleanEnd, frames };
}

export const quantile = (xs, q) => { const s = [...xs].sort((a, b) => a - b); return s[Math.min(s.length - 1, Math.floor(q * s.length))]; };
export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
