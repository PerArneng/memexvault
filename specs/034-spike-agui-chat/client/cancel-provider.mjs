// Check 3's provider-side evidence. Fake upstream: when does the connection really close? vLLM: does the output token counter stop?
import { mkdirSync, writeFileSync } from "node:fs";
import { BASE, run, sleep } from "./lib.mjs";

const out = {};

// The upstream sees its caller hang up only if ADK's HTTP client really closed the connection.
const fake = await run({ provider: "fake-slow", prompt: "Hi", abortAfterDeltas: 5 });
await sleep(300);
const closed = (await (await fetch(`${BASE}/spend`)).json()).upstream_closed_ms;
out.fakeUpstream = { closedMsAfterAbort: closed === null ? null : Math.round(closed - fake.abortWall), deltas: fake.deltas.length };
console.log("fake upstream", out.fakeUpstream);

// vLLM's router counters don't move for streams (a finished 200-token stream added 0), so they can't show this.
// Contention can: a probe slows down while other requests really generate. The control leaves 4 running on purpose.
const LONG = "Write a 3000 word story about the fictional Zorblat Bridge in the made-up town of Quennet.";
const PROBE_TOKENS = 150;
async function probe() {
  const r = await run({ provider: "local", prompt: LONG, maxOutputTokens: PROBE_TOKENS });
  const ms = r.totalMs - r.firstDeltaMs;
  return { firstDeltaMs: Math.round(r.firstDeltaMs), tokensPerS: Math.round((r.server.output_tokens / ms) * 1000 * 10) / 10 };
}
out.baseline = [await probe(), await probe()];

// Arm 1: abort 4 runs of 4000 tokens each after 5 deltas, then probe at once. Uncancelled they would run for 3 minutes.
const aborted = await Promise.all([1, 2, 3, 4].map(() => run({ provider: "local", prompt: LONG, maxOutputTokens: 4000, abortAfterDeltas: 5 })));
out.afterAborts = { aborted: aborted.map((a) => a.deltas.length), probes: [await probe(), await probe()] };
await sleep(2000);

// Arm 2, the control: 4 streams left running through the probe. They end by themselves at 800 tokens.
const running = [1, 2, 3, 4].map(() => run({ provider: "local", prompt: LONG, maxOutputTokens: 800 }));
await sleep(1000);
out.whileRunning = { probes: [await probe(), await probe()] };
const finished = await Promise.all(running);
out.whileRunning.controlOutputs = finished.map((r) => r.server.output_tokens);
console.log(JSON.stringify(out, null, 1));
mkdirSync("../out", { recursive: true });
writeFileSync("../out/cancel-provider.json", JSON.stringify(out, null, 2));
