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
Done on `PerArneng/spec-50-chat-ui`. Every item except item 1's live-model run passes. The live run calls a provider and costs money, so it wasn't run.

**Built.**
- `core`: `AgentChannel`, a pure `transcriptReducer`, citation resolution (`citationIndex`, `prepareReply`), and `createChatSession` and `createChatCases` on the `Engine`. `scriptedAgentChannel` is in `core/testing`.
- `api`: the chat DTO translators, including tool answers: `search`, `segment`, `claim`, `proposal`, `object`, and `error`, which also covers the runner's bare `{error: "Tool propose not found"}` for a tool the model wasn't offered. Anything else reads as `other`.
- `edge-agui`: `createAgUiAgentChannel` over `@ag-ui/client`'s `HttpAgent`, which sends the CSRF header and only the new message.
  - Stop calls `abortRun()` and shows as stopped. An HTTP refusal becomes `refused`, and a lost stream becomes `dropped`.
  - Zod is set to jitless before `@ag-ui/client` loads. Otherwise Zod 4's eval probe, which `@ag-ui/core`'s schemas run as they're built, trips the console's CSP.
- Console:
  - A lazy chat panel beside every screen (`chat-toggle`) and at `/vaults/{id}/chat`, with threads, streaming, Stop and tool steps.
  - Citation chips open the viewer at the segment, and unresolved citations are struck through and counted.
  - Partial replies are marked by how they ended.
  - Proposal cards use the review API, and the banner and chat-off states show model and budget.
  - The current screen's object reaches the chat only through a checkbox.
- Backend, for the banner: `GET /chat/settings` gains two fields.
  - `model`, which every reader of the vault's chat settings gets. It's null when the server has no chat model.
  - `endpoint`, `scheme://host[:port]` with no path, query or userinfo, which only an admin of the vault gets. Everyone else gets `endpoint: null`, and the API docs say so.
- Backend fix (spec 049's ADK edge): `memex-agent-adk` called `ChatHost::event` on its runtime's thread. On Postgres, a tool's answer makes `ChatRun` flush to the store, and the blocking client panicked ("Cannot start a runtime from within a runtime"), which aborted `serve` on the first tool call.
  - The edge now runs `event` through `spawn_blocking`, as it already did `before_call` and `call_tool`.
  - The test host now blocks the same way. Without the fix, 7 of its 16 tests fail with that panic.
  - Neither `chat_adk` (memory store) nor the `pg_` chat tests (scripted model) took this path.

**Decision (the user's).**
- The banner names the model to everyone and the endpoint only to admins, so the endpoint's host isn't shown to every reader.
- The endpoint is reduced to its origin so a key in a path or query can't leak.
- `the_settings_name_the_model_to_readers_and_the_endpoint_only_to_admins` checks reader, proposer, admin and root, and that a reader's response never holds the host. In the browser, item 1 checks that a reader sees the model and no endpoint.

**Measured.** The browser checks ran against `memexvault serve` on its own database, with a stand-in model on this machine. Six workers shared 16 cores, and the load averages are given with each run. All 6 checks passed on two runs in a row: 1.7 min at load 14.6–18.5 and 1.3 min at load 11.9–12.3. The figures below are from the second run.
- Setup: the 12-page PDF gave 12 segments, each on its own page.
  - A segment closes at 100 estimated tokens, so each test page carries about 140 words.
  - The first try had 25-word pages, which merged into 3 segments.
- Item 1:
  - Chat off, then on, and the banner names `stand-in-chat` and the stand-in's origin to an admin.
  - A streamed reply with a `search` step marked done, 2 citation chips and 1 unresolved citation marked.
  - The thread reads back the same after a reload.
  - A reader sees the model and no endpoint, and the API gives the reader `endpoint: null`.
  - No console errors.
- Item 2: 10 of 10 citations opened the cited segment on its page (pages 1–10), with the segment located and its region highlighted.
- Item 3: hostile fixtures in a document and in a reply loaded nothing from outside.
  - In the panel: 0 scripts, images, frames, styles or SVGs, 0 `javascript:` links, and 2 links blocked.
  - `window.__pwned` stayed unset, the canary got 0 hits, and the network log had 0 outside requests.
- Item 4: approving from the card and approving in the review screen gave the same `state` (committed), `decided_by` (the admin), `decision_reason`, `proposer` (`chat-agent`) and `requested_by`.
  - A propose-scope user's card shows no buttons and says deciding needs admin scope. Their proposal stays pending.
  - A reader is never offered `propose`, so the step fails and there's no card.
- Item 5: Stop over 5 rounds, measured in the page.
  - It took 9–12 ms to show as stopped in the second run (9–51 ms in the first), against a limit of 1,000 ms.
  - The server closed the model's stream 66–72 ms after the click (68–169 ms in the first).
  - The partial reply stays marked stopped after a reload.
- Item 6: the reducer and citation tests cover each event kind:
  - out-of-order text and tools, repeated and unknown events, tool errors
  - `RUN_ERROR` with and without an open message, and abort while stopping
  - stale run ids, a dropped connection and refusals

  That's 19 tests in `core/src/chat.test.ts`, 8 in `chat-cases.test.ts` and 8 translator tests in `api`. `edge-agui`'s 35 tests run every stream in `crates/memex-http/tests/fixtures/agui` through the real `HttpAgent` and the reducer, plus refusal, a dropped connection, and Stop both mid-stream and before the first byte.
- Item 7:
  - dependency-cruiser allows `@ag-ui/client` and `rxjs` only in `edge-agui` (0 violations, 235 modules), and the browser check scans every package's `package.json`.
  - The AG-UI chunk is 285,378 B (63,362 B gzip). It's absent from `index.html` and loads on the first message.
  - The page loaded 19 scripts before the chat opened, 3 when it opened (the panel's) and 1 on the first send (AG-UI).
  - The chat panel's chunk is 17,788 B (6,267 B gzip).
- Backend checks:
  - `cargo build`, `fmt --check` and `clippy --all-targets -D warnings` pass for the workspace.
  - `cargo test --workspace`: 995 passed, 0 failed, 108 ignored.
  - The Postgres chat suite (`--test chat ... pg_`): 5 of 5 passed.
- Console checks:
  - `pnpm test`: 759 tests in 45 files pass.
  - typecheck, lint, build, licenses (409 packages) and contrast (53 pairs, 0 failed) pass.
  - Initial JS is 148,842 B gzip of the 200,000 budget.

**Not run.**
- Item 1's live-model run: it calls a provider and costs money. Spec 049's Result has the backend's command, and the console needs only `MEMEX_CHAT_MODEL` set to a real provider's model.
- `cargo deny check licenses`: `cargo-deny` isn't installed on this machine.
- The search and vector tests that need spec 004/005's data and the e5 and reranker files aren't on this machine.

**Follow-ups.**
- A settings screen for turning chat on per vault. Until then, the chat-off state tells admins to use the API.
- A reader's chat gets "Tool propose not found" from the runner, as the tool isn't offered. A clearer message from the server would read better.
