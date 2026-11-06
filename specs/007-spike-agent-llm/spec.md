---
id: 007
name: spike-agent-llm
description: Prove ADK-Rust can run schema-constrained agents on both Gemini and a local OpenAI-compatible model
---

# 007 spike-agent-llm

## Goal
Confirm ADK-Rust fits behind our `AgentRunner` trait, and record its exact version and the trait shape. Pass: at least
99% schema-valid replies after one retry on both Gemini and the local vLLM endpoint.

## Background
**Depends on:** none. Design: `docs/architecture.md`, sections Decisions, Prompt injection.

Use the `adk-rust-docs` skill rather than memory. On 2026-11-05 crates.io lists `adk-rust` 2.3.0 as the latest while the
skill mentions 3.x, so check what is really published. Use the `vllm-local` skill before calling the local model. It
documents a reasoning-field trap that returns empty content. The Gemini key comes from `.env` (`GEMINI_API_KEY`) and must
never be logged or committed.

## Approach
- Throwaway crate in this spec's folder.
- 100 synthetic extraction prompts built from made-up text, so no real documents leave the machine.
- Run an agent with no tools and schema-only output on each provider. Measure valid replies, retries, latency, token
  counts, timeout behaviour and behaviour when the endpoint is down.
- Set a spending cap before live runs, and report the actual token use.
- Sketch `AgentRunner` (input, schema, budget, output, usage, error kinds) and check it can be faked in pure tests.

## Verification
1. Results table per provider: valid on first try, valid after one retry, p50 and p95 latency, tokens.
2. Both reach ≥99% valid after one retry. If vLLM isn't reachable, put the spec on hold rather than passing it on Gemini
   alone.
3. An empty-content reply from the reasoning-field trap is handled or reported as a typed error.
4. A confirmed zero-tool setup: a prompt asking for a tool call produces none.
5. The sketch of `AgentRunner` exposes no ADK type.
6. `docs/architecture.md` records the pinned version.

If ADK-Rust can't do this, put the spec on hold and ask the user, since the runtime was the user's choice.

## Out of scope
Extraction prompts and quality (spec 024), and sessions, memory and voice features of ADK-Rust.

## Result
**Decision: keep ADK-Rust, pinned to `=2.3.0`** (`adk-agent`, `adk-core`, `adk-model` with `gemini` and `openai`, `adk-runner`, `adk-session`), behind our own `AgentRunner`. 3.x isn't published on crates.io. Both providers passed at 100%.

100 synthetic extraction prompts, 25 in each of English, Swedish, German and Spanish, with made-up people, products and places, and a schema requiring a language enum and at least one claim with a quote. No real documents left the machine. The Gemini key came from `.env` and wasn't logged. Spending cap: the bench stops at 150,000 tokens for Gemini and 400,000 for vLLM.

| Provider | Model | Valid first try | Valid after one retry | p50 | p95 | Tokens in / out | Quotes verbatim |
|---|---|---|---|---|---|---|---|
| Gemini | `gemini-3.5-flash-lite` | 100/100 | 100/100 | 1.19 s | 1.50 s | 21,121 / 26,299 | 461/474 |
| vLLM | `qwen3.6-35b`, thinking off | 100/100 | 100/100 | 16.3 s | 23.5 s | 23,721 / 37,937 | 619/620 |

Sequential requests. Latency is per reply, and the local model's cost is hardware time, about 16 s per reply here. Quote verification (spec 013) is needed, as 13 of 474 Gemini quotes weren't verbatim.

**Verification.**
1. Table above.
2. Both reach ≥99%. The retry path didn't run live, as nothing failed, and it's tested with a fake.
3. Reasoning trap: with thinking on and 24 tokens allowed, the runner returns `Truncated { usage }`, since `finish_reason` is `length`. An empty reply with another finish reason returns `EmptyReply`. Both are retried once by the policy. With thinking on and room, the same prompt took 129 s and 2,821 tokens against about 16 s and 380 with it off, so **extraction must turn thinking off**. ADK has no option for that, and it goes in through `with_request_adapter`, which adds `chat_template_kwargs`.
4. Zero tools: a prompt demanding `fetch_url`, `web_search` and `delete_all` calls gave `tool_calls: 0` on both providers and valid JSON with one claim.
5. The `AgentRunner` sketch in `bench/src/runner.rs` is synchronous and plain, with no ADK type (grep finds none). Four tests run it on a scripted fake. ADK is confined to `bench/src/adk.rs`.
6. `docs/architecture.md` records the pinned version.

**Other behaviour.** A 5 ms timeout returned `Timeout` in 6 to 9 ms. An endpoint that's down returned `Unavailable` in 1.76 s, including the client's retries. The binary links system libraries only.

**Found.**
- Gemini's native `response_schema` rejects `additionalProperties` with a 400. The runner strips it from what's sent and still enforces it with its own validator. Spec 024 needs this for dynamic entity schemas, which may use other keywords Gemini refuses. Those failures are typed as `Provider` errors and don't retry.
- ADK's `output_schema` has built-in retries, default 3. We set `output_max_retries(0)` and keep retry as our own pure policy, so attempts and usage are counted in one place.
- vLLM's usage reports reasoning as output tokens, and `thinking_tokens` stays 0. Cost reports can't split them.
- Not measured: streaming deltas, which belong to spec 034.

This spec's measurements ran while spec 004's embedding job was in `working`, because that job needed no attention and these calls use another machine. It went through `working` afterwards.
