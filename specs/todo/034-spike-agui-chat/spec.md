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
Not started.
