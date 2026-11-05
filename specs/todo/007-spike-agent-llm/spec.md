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
Not started.
