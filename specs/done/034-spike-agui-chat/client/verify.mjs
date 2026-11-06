// Checks 1 to 4 of the spec. Usage: node verify.mjs scripted | live. Live also needs vLLM and the server's GEMINI_API_KEY.
import { mkdirSync, writeFileSync } from "node:fs";
import { BASE, rawRun, run, sleep, stats } from "./lib.mjs";

const live = process.argv[2] === "live";
const results = [];
const check = (name, ok, detail = {}) => { results.push({ name, ok, ...detail }); console.log(`${ok ? "PASS" : "FAIL"} ${name} ${JSON.stringify(detail)}`); };

const STORY = "Write a 200 word description of the fictional Zorblat Bridge in the made-up town of Quennet.";
const LONG = "Write a 3000 word story about the fictional Zorblat Bridge in the made-up town of Quennet.";
const TOOL = "Use the lookup_fact tool with the topic bridge, then tell me the fact in one sentence.";
const liveProviders = live ? ["local", "gemini"] : [];

// 1. Deltas, and exact assembly by the client.
for (const provider of ["scripted", "fake", ...liveProviders]) {
  const r = await run({ provider, prompt: STORY });
  const joined = r.deltas.join("");
  const ok = r.deltas.length >= 5 && r.invalid.length === 0 && !r.thrown && r.assistantText === joined && joined === r.server.final_text && joined.length > 0;
  check(`1 deltas ${provider}`, ok, { deltas: r.deltas.length, chars: joined.length, sameAsAdkFinal: joined === r.server.final_text, firstDeltaMs: Math.round(r.firstDeltaMs), totalMs: Math.round(r.totalMs), tokens: [r.server.input_tokens, r.server.output_tokens], thrown: r.thrown });
}

// 2. Tool call order and schema.
for (const provider of ["scripted", ...liveProviders]) {
  const r = await run({ provider, mode: "tool", prompt: TOOL, maxOutputTokens: 300 });
  const tool = r.types.filter((t) => t.startsWith("TOOL_CALL_"));
  const start = r.events.find((e) => e.type === "TOOL_CALL_START");
  const argEvents = r.events.filter((e) => e.type === "TOOL_CALL_ARGS");
  const result = r.events.find((e) => e.type === "TOOL_CALL_RESULT");
  const sameId = start && result && result.toolCallId === start.toolCallId;
  let argsParse = false;
  try { argsParse = typeof JSON.parse(argEvents.map((e) => e.delta).join("")) === "object"; } catch { /* stays false */ }
  const ok = tool.join() === "TOOL_CALL_START,TOOL_CALL_ARGS,TOOL_CALL_END,TOOL_CALL_RESULT" && sameId && argsParse && r.invalid.length === 0 && !r.thrown && r.types.at(-1) === "RUN_FINISHED" && r.toolMessages.length === 1;
  check(`2 tool ${provider}`, ok, { tool, name: start?.toolCallName, id: start?.toolCallId, args: argEvents.map((e) => e.delta).join(""), answer: r.deltas.join("").slice(0, 120), invalid: r.invalid, thrown: r.thrown, types: ok ? undefined : r.types, tokens: [r.server.input_tokens, r.server.output_tokens] });
}

// 3. Cancel. The clock is shared, as both ends run on this machine.
async function cancelCase(name, spec, end) {
  const r = await run(spec);
  let s = r.server;
  for (let i = 0; i < 100 && (s?.[end] == null); i++) { await sleep(50); s = await stats(r.runId); }
  const stopMs = s?.[end] == null ? null : s[end] - r.abortWall;
  const detail = { stopMs: stopMs === null ? null : Math.round(stopMs), modelEnd: s?.model_end, deltas: r.deltas.length, chunks: s?.chunks, chunksAfterDisconnect: s?.chunks_after_disconnect, disconnectMs: s?.disconnect_ms == null ? null : Math.round(s.disconnect_ms - r.abortWall), invalid: r.invalid.length, last: r.types.at(-1), thrown: r.thrown };
  // The end must follow the abort, or the model had already finished and cancel proved nothing.
  check(`3 cancel ${name}`, stopMs !== null && stopMs >= 0 && stopMs <= 1000 && r.abortWall !== null && r.invalid.length === 0, detail);
}
await cancelCase("scripted, during text", { provider: "scripted", mode: "slow", prompt: "Hi", abortAfterDeltas: 5 }, "model_end_ms");
await cancelCase("scripted, during a 30 s tool", { provider: "scripted", mode: "slowtool", prompt: "Hi", abortAfterMs: 2500 }, "tool_dropped_ms");
if (live) {
  await cancelCase("local", { provider: "local", prompt: LONG, maxOutputTokens: 4000, abortAfterDeltas: 5 }, "model_end_ms");
  await cancelCase("gemini", { provider: "gemini", prompt: LONG, maxOutputTokens: 4000, abortAfterDeltas: 5 }, "model_end_ms");
}

// 4. Failures become RUN_ERROR and a clean end, through HttpAgent and on the wire.
for (const [provider, mode] of [["scripted", "fail"], ["scripted", "errchunk"], ["fake-cut", "chat"], ["fake-500", "chat"], ["down", "chat"], ["bad-key", "chat"]]) {
  const r = await run({ provider, mode, prompt: "Hi" });
  const w = await rawRun({ provider, mode });
  const last = r.events.at(-1);
  const ok = last?.type === "RUN_ERROR" && r.invalid.length === 0 && w.cleanEnd && w.status === 200 && w.frames.at(-1)?.type === "RUN_ERROR" && w.frames.filter((f) => f.type === "RUN_ERROR").length === 1 && !w.frames.some((f) => f.type === "RUN_FINISHED");
  check(`4 failure ${provider}/${mode}`, ok, { types: [...new Set(r.types)], code: last?.code, message: last?.message, totalMs: Math.round(r.totalMs), wireCleanEnd: w.cleanEnd, wireLast: w.frames.at(-1)?.type, contentType: w.contentType, clientThrew: r.thrown, serverDetail: r.server?.error_detail?.slice(0, 120) });
}

if (live) console.log("spend", await (await fetch(`${BASE}/spend`)).json());
mkdirSync("../out", { recursive: true });
writeFileSync(`../out/verify-${live ? "live" : "scripted"}.json`, JSON.stringify(results, null, 2));
const failed = results.filter((r) => !r.ok);
console.log(`${results.length - failed.length}/${results.length} passed`);
process.exit(failed.length ? 1 : 0);
