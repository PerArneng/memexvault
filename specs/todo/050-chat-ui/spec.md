---
id: 050
name: chat-ui
description: Build the AG-UI chat with streaming, citations that open sources, and proposal cards
---

# 050 chat-ui

## Goal
A user chats with the agent and sees answers stream in, what the agent is doing, and citations that open the exact source region. Proposals can be approved from the chat through the normal review rules, and hostile content can't run script or load remote content.

## Background
**Depends on:** 042, 046, 049. Design: `docs/frontend-architecture.md`, sections Decisions, Security.

## Approach
- `AgentChannel` interface in `core`, `AgUiAgentChannel` in `edge-agui`, and `ScriptedAgentChannel` for tests. A pure reducer builds the transcript from protocol events.
- A chat panel with streaming text, a stop button, and thread history. It opens as a side panel from any screen, and it only sees the current screen's object if the user adds it.
- Tool activity shown as compact steps, with search queries and objects read.
- Citations as chips that open the viewer at the exact region, or a time range. Reply text with citations that don't resolve is visibly marked.
- Proposal cards showing the change with evidence, and approve and reject buttons that call the review API, shown only to users allowed to decide.
- A banner stating the model and endpoint in use, and budget state. A clear state when chat is off for the vault.

## Verification
1. A streamed reply, tool steps, citations and a proposal card pass in Playwright with a scripted model against the real backend, and a run against a live model is recorded once using public fixtures.
2. A click on 10 citations opens the correct regions.
3. Hostile fixtures in model replies and tool results (scripts, `javascript:` links, remote images, HTML tags) run nothing and load nothing from outside, shown by a network log and script check.
4. Approving a card has the same effect and attribution as approving in the review screen, and a read-scope user sees no buttons.
5. Stop ends the stream within 1 s, and the partial reply stays marked as stopped.
6. The reducer is unit-tested for each event kind, including out-of-order events, unknown events, tool errors, `RUN_ERROR` and a dropped connection.
7. `rxjs` and `@ag-ui/client` appear only in `edge-agui`'s dependencies, and the chat code loads only when opened.

## Out of scope
Voice, generative UI from the agent, shared threads and file upload in chat.

## Result
Not started.
