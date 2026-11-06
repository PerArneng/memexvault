---
id: 049
name: chat-agent-backend
description: Add the chat agent with threads, read and propose tools, and an AG-UI endpoint
---

# 049 chat-agent-backend

## Goal
An agent that answers from the vault with citations and can propose changes, streamed over AG-UI. Injection fixtures commit nothing, and it can't do anything a `propose`-scoped client can't do.

## Background
**Depends on:** 013, 017, 020, 034. Design: `docs/architecture.md`, sections Chat agent, Prompt injection.

Spec 034's Result lists what this spec inherits: terminal events repeat content, output guardrails turn streaming off, and cancelled runs' token use, thinking and `MaxTokens` weren't measured.

## Approach
- `AgentRunner` gains streaming events. A pure mapping to AG-UI uses the result of spec 034, with `memex-agent-adk` still the only crate depending on ADK.
- `POST /api/v1/vaults/{id}/chat/runs` as server-sent AG-UI events, and routes for thread list, read, rename and delete. Threads belong to a principal, and are stored in the vault's schema.
- Tools: search, read object, provenance, and propose. No URL fetching. Tool descriptions are static, and tool results are clearly marked as data.
- `propose` submits an ordinary proposal attributed to the agent and the requesting user. Its tool result is the proposal id, and the run continues without waiting.
- A per-thread and per-day token budget, with cancel support. Runs record Derivations with token counts.
- A per-vault setting, off by default, and enabling it requires admin scope. Add chat threads to the erasure cascade from spec 030.

## Verification
1. On 20 golden-set questions with a live model, every factual sentence in the reply cites a resolvable anchor, or the reply says it found nothing. Results are recorded. Public fixtures only.
2. Injection fixtures: documents telling the agent to approve, write, fetch URLs or reveal its prompt lead to 0 commits and 0 outbound requests other than to the LLM.
3. A token with `read` scope can chat but can't get a proposal through, and the agent never exceeds the requesting user's scope.
4. A proposal from chat goes through validators and the review queue with both attributions shown.
5. Chat disabled on a vault refuses runs with a clear error, and nothing reaches the LLM, proven with a scripted fake.
6. Erasing a source deletes threads citing it, and a raw scan finds no marker in chat tables after compaction.
7. The budget stops a run cleanly, and cancel stops token use within 1 s.
8. The AG-UI stream passes `@ag-ui/core` schema validation in every scripted scenario, including provider failures.
9. A principal can't read another principal's threads.

## Out of scope
Agent long-term memory, sharing threads, file upload in chat, voice, AG-UI interrupts and direct writes.

## Result
Not started.
