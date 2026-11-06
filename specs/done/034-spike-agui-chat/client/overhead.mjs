// Check 5: first-token time through the bridge against a direct call to the same model from the same Node process.
// Pairs alternate which side goes first, and the first 2 pairs warm connections and are dropped. Usage: node overhead.mjs PROVIDER [PAIRS]
import { mkdirSync, writeFileSync } from "node:fs";
import { BASE, quantile, run } from "./lib.mjs";

const provider = process.argv[2];
const pairs = Number(process.argv[3] ?? 20);
const PROMPT = "Count from 1 to 10 in words.";
const INSTRUCTION = "You are a test assistant. Answer plainly.";
const MAX = 32;

const direct = {
  fake: () => [`${BASE}/fake/ok/v1/chat/completions`, {}, { model: "fake", stream: true, messages: [{ role: "user", content: PROMPT }] }, (j) => j.choices?.[0]?.delta?.content],
  local: () => ["http://vllm.brain.home.arpa/v1/chat/completions", {}, { model: "qwen3.6-35b", stream: true, max_tokens: MAX, temperature: 0, chat_template_kwargs: { enable_thinking: false }, messages: [{ role: "system", content: INSTRUCTION }, { role: "user", content: PROMPT }] }, (j) => j.choices?.[0]?.delta?.content],
  gemini: () => [`https://generativelanguage.googleapis.com/v1beta/models/${process.env.GEMINI_MODEL ?? "gemini-3.5-flash-lite"}:streamGenerateContent?alt=sse`, { "x-goog-api-key": process.env.GEMINI_API_KEY }, { systemInstruction: { parts: [{ text: INSTRUCTION }] }, contents: [{ role: "user", parts: [{ text: PROMPT }] }], generationConfig: { maxOutputTokens: MAX, temperature: 0 } }, (j) => j.candidates?.[0]?.content?.parts?.find((p) => p.text)?.text],
}[provider];

async function directCall() {
  const [url, headers, body, textOf] = direct();
  const t0 = performance.now();
  const r = await fetch(url, { method: "POST", headers: { "Content-Type": "application/json", ...headers }, body: JSON.stringify(body) });
  if (!r.ok) throw new Error(`direct call answered ${r.status}`); // Never prints headers, so the key stays out of logs.
  let first = null, buf = "";
  const decoder = new TextDecoder();
  for await (const chunk of r.body) {
    buf += decoder.decode(chunk, { stream: true }).replaceAll("\r\n", "\n"); // Gemini ends frames with CRLF.
    let i;
    while ((i = buf.indexOf("\n\n")) >= 0) {
      const frame = buf.slice(0, i); buf = buf.slice(i + 2);
      if (!frame.startsWith("data:") || frame.includes("[DONE]")) continue;
      if (first === null && textOf(JSON.parse(frame.slice(5)))) first = performance.now() - t0;
    }
  }
  if (first === null) throw new Error("no text in the direct call");
  return first;
}

async function bridged() {
  const r = await run({ provider, prompt: PROMPT, maxOutputTokens: MAX });
  if (r.firstDeltaMs === null) throw new Error(`no delta through the bridge: ${r.types}`);
  const s = r.server;
  return { ms: r.firstDeltaMs, setup: s.model_call_ms - s.request_ms, chunkToOut: s.first_delta_out_ms - s.first_chunk_ms };
}

const rows = [];
for (let i = 0; i < pairs + 2; i++) {
  let d, b;
  if (i % 2) { d = await directCall(); b = await bridged(); } else { b = await bridged(); d = await directCall(); }
  if (i >= 2) rows.push({ direct: d, bridge: b.ms, setup: b.setup, chunkToOut: b.chunkToOut });
}
const col = (k) => rows.map((r) => r[k]);
const f = (x) => Math.round(x * 10) / 10;
const summary = {
  provider, pairs,
  directP50: f(quantile(col("direct"), 0.5)), directP95: f(quantile(col("direct"), 0.95)),
  bridgeP50: f(quantile(col("bridge"), 0.5)), bridgeP95: f(quantile(col("bridge"), 0.95)),
  overheadP50: f(quantile(col("bridge"), 0.5) - quantile(col("direct"), 0.5)),
  pairedDiffP50: f(quantile(rows.map((r) => r.bridge - r.direct), 0.5)),
  serverSetupP50: f(quantile(col("setup"), 0.5)), serverSetupP95: f(quantile(col("setup"), 0.95)),
  serverChunkToOutP50: f(quantile(col("chunkToOut"), 0.5)), serverChunkToOutP95: f(quantile(col("chunkToOut"), 0.95)),
};
console.log(JSON.stringify(summary));
mkdirSync("../out", { recursive: true });
writeFileSync(`../out/overhead-${provider}.json`, JSON.stringify({ summary, rows }, null, 2));
