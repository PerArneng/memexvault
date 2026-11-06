---
id: 034
name: spike-agui-chat
description: Prove the chat path from ADK through our bridge to the AG-UI client before building on it
---

# 034 spike-agui-chat

## Goal
Show that a thin slice works end to end: an ADK agent, our mapping to AG-UI events, a server-sent stream, and `@ag-ui/client`. Pass: text arrives as
deltas, tool calls appear, cancel works, and our first-token overhead is ≤100 ms on top of the LLM's own.

## Background
**Depends on:** 007. Design: `docs/architecture.md`, section Chat agent.

ADK-Rust documents an `ag_ui` mode in its unpublished 3.0 release candidate, while 2.3.0 is the published release. The pages read on 2026-11-05 name
no text-delta event, so streaming is the main risk. AG-UI's interrupt flow is marked draft, and this design doesn't use it. The exact
`RunAgentInput` fields and the response Content-Type weren't fully stated in the pages read, so they're checked against `@ag-ui/core`'s schemas.

## Approach
- Throwaway crate and a small Node script in this spec's folder.
- Read `adk-core` and the runner to find how partial text and tool events are exposed in the chosen version.
- Write the pure mapping from runner events to AG-UI events, in the order the protocol requires, with a golden event sequence.
- Serve it from a small axum route over server-sent events. Run it with a scripted model, Gemini with a spending cap, and the local OpenAI-compatible
  server. Use synthetic prompts only.
- Consume it with `HttpAgent` from `@ag-ui/client`, validating every event against the `@ag-ui/core` schemas.

## Verification
1. A text reply arrives as at least 5 separate deltas with the live models, and the client assembles it exactly.
2. Tool call start, args, end and result arrive in order, and every event passes schema validation.
3. Cancel from the client stops the model call within 1 s, shown by token use on the provider side or the server log.
4. A provider failure becomes a `RUN_ERROR` event and a clean end of stream.
5. First-token overhead is ≤100 ms, measured against a direct call to the same model.
6. The Result records the ADK version to use, and whether using an unpublished release is needed. Using one is the user's call.

If ADK can't stream text deltas in a published version, put the spec on hold and ask the user, naming the options: an unpublished version, or
sending whole messages without deltas.

## Out of scope
Threads, the product tools, the chat UI (050) and AG-UI interrupts.

## Result
**Decision: keep the design. Use ADK-Rust `=2.3.0`, the published release, with no unpublished version needed.** ADK's own `ag_ui` mode isn't used. Our pure mapper does the work on top of `run_with_config` in `StreamingMode::SSE`. Client side: `@ag-ui/client` and `@ag-ui/core` 1.0.4.

Built, all throwaway: `bench/` (`mapping.rs` is the pure mapper, `adk.rs` the edge, `scripted.rs` the test models, `main.rs` the axum route, `golden/` the sequences) and `client/` (`verify.mjs`, `overhead.mjs`, `cancel-provider.mjs`). To rerun: `cargo build --release` in `bench/` and start the server, then `npm install` and `node verify.mjs live` in `client/`. Prompts were synthetic. Results land in the git-ignored `out/`.

**Verification.** `verify.mjs live` passed 17/17 on the final build, and `cargo test --release` passed 10/10, with the golden sequences produced by the real ADK runner.

1. **Deltas.** The client's assembled text was identical to the deltas and to ADK's final text on every provider.

   | Provider | Deltas | Output tokens | First delta | Whole reply |
   |---|---|---|---|---|
   | Scripted, with an emoji and accents split across chunks | 10 | n/a | 33 ms | 34 ms |
   | vLLM `qwen3.6-35b`, thinking off | 373 | 374 | 252 ms | 17.3 s |
   | Gemini `gemini-3.5-flash-lite` | 11 | 249 | 548 ms | 1.6 s |

2. **Tools.** Start, args, end and result arrived in that order with matching ids on scripted, vLLM and Gemini, and the model then answered from the result. No event failed `EventSchemas`, and `HttpAgent`'s own ordering rules threw nothing.
3. **Cancel.** The target is 1 s.

   | Case | Measure | Result |
   |---|---|---|
   | vLLM, Gemini, scripted text | Client abort to model stream dropped, 0 chunks after | 0 to 2 ms |
   | Scripted, during a 30 s tool | Client abort to tool future dropped | 1 ms |
   | Fake upstream | Client abort to the upstream seeing its connection close | 3 ms |
   | vLLM, provider side | First token of a probe right after 4 aborted 4000-token runs | 149 ms (baseline 112 to 128 ms) |
   | vLLM, control | The same probe with 4 runs left generating | 73,232 ms |

   The local engine queues requests, so the probe would have waited if the aborted runs were still generating. Gemini's side wasn't observed: the evidence there is our server log, and that our connection closed.
4. **Failures.** A scripted error, an error inside a successful chunk, a connection cut mid-stream, HTTP 500, an endpoint that's down and a rejected Gemini key each gave exactly one `RUN_ERROR`, no `RUN_FINISHED` and a clean end of body. HTTP 500 and the down endpoint take about 1.8 s because of ADK's retries.
5. **First-token overhead.** Passes at p50. Pairs alternated which side went first, and the first 2 pairs were dropped.

   | Provider | Pairs | Direct p50 / p95 | Bridge p50 / p95 | Overhead p50 | Paired difference p50 | Server's share p50 / p95 |
   |---|---|---|---|---|---|---|
   | Fake upstream | 50 | 4.1 / 59.2 ms | 3.7 / 46.1 ms | -0.3 ms | 0.7 ms | 0.1 / 1.2 ms |
   | vLLM | 20 | 111.5 / 115.5 ms | 112.9 / 246.5 ms | 1.4 ms | 1.5 ms | 0.1 / 1.0 ms |
   | Gemini | 20 | 508 / 712 ms | 552 / 978 ms | 44 ms | 31 ms | 0.2 / 0.8 ms |

   An earlier Gemini run gave -15 ms at p50, so that row is mostly network noise. **The p95 gap is above 100 ms on Gemini (266 ms, and 136 ms in the earlier run) and on vLLM in this run (131 ms, and 1 ms in the earlier run).** Our measured share at p95 is about 1 ms, and the rest wasn't attributed. It may be in ADK's HTTP client, which isn't separated from the model's own wait here, and 20 pairs make p95 close to the single worst sample. The share counts setup before the model call and first chunk to first frame written. The bridge side also includes `HttpAgent` itself.
6. **Version.** `=2.3.0`, as above.

**For spec 049.**
- A terminal event repeats the turn's text and tool calls, so a naive mapping sends everything twice. The mapper emits only what's new, and the golden tests hold it to that.
- **Output guardrails turn streaming off.** `llm_agent.rs` streams only when `output_guardrails` is empty, so adding one would silently lose the deltas.
- `model_end: dropped` also happens after a normal finish, because ADK leaves its loop on `turn_complete` before the stream ends. Don't treat a dropped stream as a cancel.
- Tool arguments arrive whole, as one `TOOL_CALL_ARGS`.
- The runner is driven from inside the response stream, so a client disconnect cancels the run. A reconnect can't resume it.
- The cancel guard must be created inside the `stream!` block. Outside it, closure capture takes only one field, the guard drops at once, and every run is cancelled. The compiler only warns about this.
- The client gets a fixed message and a category code. Provider text stays on the server. Fatal categories such as a rejected key arrive as `invalidinput`.
- Keep-alive comments are tolerated: `HttpAgent` ran through them during the slow tool, and curl saw 2 in 2.5 s. `TCP_NODELAY` is set, and wasn't compared with it off.
- The request passed `RunAgentInputSchema` with `threadId`, `runId`, `messages`, `tools`, `context`, `state` and `forwardedProps`. The response type is `text/event-stream`. The schemas are exported from `@ag-ui/core/schemas`, not the package root.
- The local engine queued the probe for 73 s behind 4 running requests, so local chat will wait behind anything else using that engine, such as extraction.

**For spec 050.**
- `abortRun()` makes the client synthesise a `RUN_ERROR` with code `abort`. The stop button must show that as stopped, not failed.
- `RUN_ERROR` can arrive with a text message still open, and the partial reply must be marked.
- Delta sizes differ widely: vLLM sends about one token per delta and Gemini about 110 characters.

**Not measured.**
- A browser. The client ran in Node.
- vLLM with thinking on. Every local run had it off. The mapper drops thinking parts, and with thinking on the first visible text would wait behind the reasoning. Spec 049 has to choose.
- Tokens used by a cancelled run. Spec 049's budget and its check 7 need this.
- A reply cut off by `MaxTokens`. The mapper ends it with a plain `RUN_FINISHED`, so the client can't tell it was cut.

**Tried and didn't work.** vLLM's router metrics can't show cancel: `num_requests_running` reads 34 when idle, and `output_tokens_total` moved 0 for a finished 200-token stream. That's why the contention check above exists.

**Cost.** About 7,000 Gemini tokens in all: 4,116 counted through the bridge, against a 60,000 cap per server start, plus about 2,800 estimated for the direct calls, which the cap doesn't cover. The aborted runs' tokens may be missing from the count. The Gemini key came from the main checkout's `.env`, because this worktree has none, and it wasn't logged or written to any file.
