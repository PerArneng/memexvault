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

From spec 034: `abortRun()` makes the client synthesise `RUN_ERROR` with code `abort`, which Stop must show as stopped. `RUN_ERROR` can also arrive with a message still open.

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
In progress on `PerArneng/spec-50-chat-ui`. The console side is built and its checks pass; the backend build and the browser checks wait for pdfium on Linux (see Not run).

**Built.**
- `core`: `AgentChannel`, a pure `transcriptReducer`, citation resolution (`citationIndex`, `prepareReply`), `createChatSession` and `createChatCases` on the `Engine`. `scriptedAgentChannel` in `core/testing`.
- `api`: the chat DTO translators, including tool answers (`search`, `segment`, `claim`, `proposal`, `object`, `error`, anything else as `other`).
- `edge-agui`: `createAgUiAgentChannel` over `@ag-ui/client`'s `HttpAgent`, sending the CSRF header and only the new message. Stop calls `abortRun()` and shows as stopped. An HTTP refusal becomes `refused`, and a lost stream becomes `dropped`.
- Console: a lazy chat panel beside every screen (`chat-toggle`) and at `/vaults/{id}/chat`, with threads, streaming, Stop, tool steps, citation chips that open the viewer at the segment, unresolved citations struck through and counted, partial replies marked by how they ended, proposal cards that use the review API, and a banner and chat-off states. The current screen's object reaches the chat only through a checkbox.
- Backend, for the banner: `GET /chat/settings` gains `model`, which every reader of the vault's chat settings gets (null when the server has no chat model), and `endpoint`, `scheme://host[:port]` with no path, query or userinfo, which only an admin of the vault gets. Everyone else gets `endpoint: null`, and the API docs say so.

**Decision (the user's).** The banner names the model to everyone and the endpoint only to admins, so the endpoint's host isn't shown to every reader. The endpoint is reduced to its origin so a key in a path or query can't leak. `the_settings_name_the_model_to_readers_and_the_endpoint_only_to_admins` checks reader, proposer, admin and root, and that a reader's response never holds the host. The banner shows the endpoint only when it is present.

**Measured.**
- Item 6: 19 reducer and citation tests in `core/src/chat.test.ts` (each event kind, out-of-order text and tools, repeated and unknown events, tool errors, `RUN_ERROR` with and without an open message, abort while stopping, stale run ids, dropped connection, refusals), 8 in `chat-cases.test.ts`, and 8 translator tests in `api`. `edge-agui`'s tests run every stream in `crates/memex-http/tests/fixtures/agui` through the real `HttpAgent` and the reducer, plus refusal, dropped, and Stop mid-stream and before the first byte: 35 tests.
- Item 7: dependency-cruiser allows `@ag-ui/client` and `rxjs` only in `edge-agui` (0 violations, 230 modules). The AG-UI chunk is 285,370 B (63,399 B gzip), loaded by dynamic import on the first message and absent from `index.html`. The chat panel's chunk is 17.78 kB (6.27 kB gzip). Initial JS is 148,845 B gzip of the 200,000 budget.
- `pnpm test` 759 tests in 45 files; typecheck, lint, build, licenses (409 packages) and contrast (53 pairs, 0 failed) pass.

**Not run.**
- Items 1–5 and 7's browser part (`e2e:chat`), and `cargo build`, `test` and `clippy` for the workspace: `scripts/fetch-pdfium.sh` has no Linux pdfium pinned, and the fix (dynamic linking on Linux) isn't on `main` yet.
- Item 1's live-model run: it calls a provider and costs money, so it was not run. Spec 049's Result has the command for the backend; the console needs only `MEMEX_CHAT_MODEL` on a real provider.

**Follow-ups.**
- A settings screen for turning chat on per vault; until then the chat-off state tells admins to use the API.

