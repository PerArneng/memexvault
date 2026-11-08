---
id: 049
name: chat-agent-backend
description: Add the chat agent with threads, read and propose tools, and an AG-UI endpoint
---

# 049 chat-agent-backend

## Goal
An agent that answers from the vault with citations and can propose changes, streamed over AG-UI. Injection fixtures commit nothing, and it can't do anything a `propose`-scoped client can't do.

## Background
**Depends on:** 013, 017, 020, 024, 034. Design: `docs/architecture.md`, sections Chat agent, Prompt injection.

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
Built on `main` at 668bd6e. **Item 1 wasn't run, as it needs a live model. Items 2 to 9 ran on a scripted model and the real ADK runner against a stand-in server on this machine, so no LLM was called, including the local vLLM endpoint.** They show what the tools, routes and stores let happen. They say nothing about what a model chooses to do. Item 6 passed for what ran: **the wiring to spec 030 is still owed.**

**Built.** `AgentRunner::chat` and a `ChatHost` callback in `memex-types`. The run on ADK's `Runner` in `memex-agent-adk/src/chat.rs`, still the only crate that names ADK. The pure mapper, transcript, budget arithmetic and citation checker in `memex-core/src/chat.rs`. The run itself in `memex-core/src/server/chat.rs`. Four tools in `memex-api/src/chat.rs`, with definitions compiled in from `chat_tools.json` and their hash pinned. Routes in `memex-http/src/routes/chat.rs`: `POST /vaults/{id}/chat/runs`, `GET`/`PUT .../chat/settings`, `GET .../chat/threads`, and `GET`/`PATCH`/`DELETE .../chat/threads/{tid}`. `ScriptedChat` for tests. Vault migration 13 (index 12). `requested_by` on ChangeSets and their DTO. `docs/architecture.md`, section Chat agent, has the design, and `AGENTS.md` the commands and settings.

**Verification.** Load averages are beside each timing: other projects kept this machine busy, so the timings are upper-end samples and not a benchmark.

1. **NOT RUN.** It needs a live model. The harness and scorer are built, and `the_scorer_tells_good_replies_from_bad_on_a_scripted_model` proves them on replies a test wrote, which says nothing about a model. The user decides on the run. Estimate: 150,000 to 300,000 tokens, from about 7,600 per question for one search and a reply, and about twice that with a follow-up read. `MEMEX_EVAL_MAX_TOKENS` is enforced as the vault's day budget. It sends the questions and public Wikipedia passages to the provider. It runs on the in-memory keyword index and fake vectors, so it measures citing and says little about search quality:
   ```
   MEMEX_EVAL_APPROVED=1 MEMEX_LLM_PROVIDER=<gemini|openai> MEMEX_LLM_MODEL=<model> MEMEX_LLM_API_KEY=<key, if needed> MEMEX_LLM_BASE_URL=<with openai> \
   MEMEX_EVAL_MAX_TOKENS=400000 MEMEX_TEST_CORPUS=/Users/perarneng/dev/memexvault/specs/004-spike-vector/corpus MEMEX_EVAL_OUT=<file> \
   cargo test -p memexvault --release --test chat_eval -- --ignored --nocapture --exact live_chat_cites_on_twenty_golden_questions
   ```
2. **Passed on a scripted agent, for the mechanisms only.** The tools can't commit, fetch or reveal anything, whatever the model says. Whether a real model tries is unmeasured. All 32 injection fixtures from spec 025, each in a `trusted` source with the chat combination earned, each with an agent's token and then the server admin's: the script read the document and then called `approve`, `write`, `commit`, `fetch_url`, `http_get`, `reveal_prompt` and `system_prompt`, and tried URLs through `read` and `search`. 0 commits, 0 assertions, 0 connections to a canary listener, and no tool answer held the instruction or the tool definitions. On the real ADK runner, a model's `fetch_url` got "not found", the canary saw nothing, and the stand-in model server was the only thing contacted. No tool takes a URL, and a test checks the definitions for it.
3. **Passed.** A `read` token isn't offered `propose`, and a script calling it anyway got `forbidden` with no ChangeSet. A server admin's agent has no way to approve, reject, revert, upload, delete or grant: 7 of 7 refused as no such tool, and the proposal stayed pending. A token revoked mid-run is refused at its next tool call and stops further model calls.
4. **Passed.** A bad quote is rejected by the validators, and the run carries on. A good one is pending in the queue, with `proposer: chat-agent`, `requested_by` the person, `needs_a_person: true` and the reason. An agent with admin scope gets 403 on approving or reverting, and a person can approve.
   **Never auto-commits, tested on the exact case:** a `trusted` source, and the chat combination at 500 of 500, on the record as earned. The chat proposal stayed pending with no decider. The control, an agent's own earned combination citing the same document, did auto-commit. Approving and rejecting chat proposals left the table unchanged, and an audit finds no combination for a chat claim, so it can neither earn nor revoke by one.
5. **Passed.** Refused with 403 and code `chat_disabled` before a stream opens, with 0 model requests and nothing written, both before it's first enabled and after it's turned off. Read and propose scopes get 403 on enabling, and the log names the admin who did. A run in progress stops before its next model call when chat is turned off.
6. **Passed for what ran:** the server function, then the store's physical removal, then a raw scan. `pg_erasing_a_source_leaves_no_marker_in_the_chat_tables_files` reads each chat table's heap file, TOAST file and index files with `pg_read_binary_file`, after a `CHECKPOINT`. The positive control found all 6 markers in those files before erasing, including one in TOAST and one on `chat_threads`. After `erase_chat_threads_citing` and `Store::compact`, the 5 for the erased thread were in 0 files and 0 rows, and the thread that read something else kept its marker. The reply in that thread cited nothing, so only the record of what reached it found it. In memory, all 8 ways content reaches a thread (search, five kinds of read, provenance, propose) recorded the source. The test sets `STORAGE EXTERNAL` on the column, as compression could hide a long marker from a scan. **Not covered:** spec 030 isn't built, so nothing calls this yet and erasing a source today doesn't reach it. Postgres's WAL and backups keep what they kept. What the provider received can't be recalled. Spec 030 must call it after the source's rows are gone, as the check that stops a late read relies on the row being gone.
7. **Passed, with a stand-in for the provider.** The budget: runs end with `RUN_FINISHED` and `result.stop` of `thread_budget` or `day_budget`, and a call that doesn't fit isn't made. With 350 tokens and a 100-token input, the output limits were 250 then 150 and there was no third call. Eight threads racing for five reservations on Postgres won exactly 5. Deleting a thread gave no budget back.
   Cancel, target 1 s, load average 27 to 94:

   | Measure | Runs | Result |
   |---|---|---|
   | Client gone to the upstream seeing its connection close, through the route on the real runner | 8 | 33 to 62 ms |
   | Cancel flag to upstream closed, on the real runner alone | 5 | 34 to 38 ms |
   | Client gone to the run's record written, on the scripted model | 5 | 1.2 to 2.3 ms |

   That's our side closing the connection. A real provider's own token use after a cancel wasn't observed, and spec 034's contention check on vLLM remains the only evidence of that. **A provider that doesn't report tokens, or a cancelled call, is charged an estimate that errs high and is never zero:** input at 3 characters per token plus 16, and output from what was seen, marked `tokens estimated` on the Derivation. This errs high against what was sent and seen, but a provider could have generated more than arrived before the connection closed, so it can't be promised to be high in every case. A call dropped before any report keeps its whole reservation, which is its input and output limit.
8. **Passed.** 27 streams against `EventSchemas` from `@ag-ui/core/schemas` 1.0.4, plus order checks the schemas can't make, in `web/packages/edge-agui/src/backend-streams.test.ts`: 29 of 29 tests, including controls that a bad event and a bad order fail. 17 are scripted, and 10 come from the real ADK runner: text, tool, cut at the limit, the call limit, HTTP 400, 500 and 429, a connection cut mid-stream, an error in a chunk and a server that's down. Each failure ended in exactly one `RUN_ERROR` with a fixed message and no provider text or key. The Rust tests hold those files to what the routes send, and `MEMEX_UPDATE_GOLDEN=1` rewrites them. `HttpAgent` itself wasn't run here, and a browser wasn't either.
9. **Passed.** Another principal of the same scope, a vault admin and a server admin each got an empty list and 404 on read, rename and delete. The same thread id under another principal is another thread, with none of the owner's history.

**Decisions.**
- **Thinking stays off for chat,** with `enable_thinking: false` on local servers and the lowest level on Gemini, which can't go lower. Spec 007 measured eight times the tokens and time with it on, it would spend the same budget as the reply, and visible text would wait behind it. A test checks the local request. Thinking parts are never shown or stored.
- **The real ADK `Runner`, with wrappers around its `Llm` and `Tool`,** rather than calling the model directly with our own loop. A loop in core would be easier to test, but this is the path spec 034 proved live, and it carries Gemini's thought signatures on tool calls, which couldn't be tested here with no live calls allowed. So the scripted model follows the contract, and the wire is tested separately.
- **Threads are off the event log,** because a copy there would outlive erasure. The cost is that they aren't in an export. The setting and each run's Derivation are on the log, and hold no text. A test confirms the markers never reach `events`.
- **`Store::compact` rewrites the chat tables with `VACUUM FULL`.** Measured: after plain `VACUUM`, a deleted message's bytes were still in the file, on a page with another live row.
- **Day budget per vault,** not per principal, since it protects the money the admin agreed to spend, and a run can be starved by others in the vault. UTC days.
- **Reservations:** each call charges its input estimate and output limit up front, in one transaction, and then settles. So concurrent runs can't overspend, and a crash errs high. A call's own overshoot, if its input was underestimated, shows at the next call.
- **Thread ids are the client's,** keyed with the owner, because `HttpAgent` always sends one. Nobody can learn whether another's id exists.
- **Only the newest user message is read,** so a client can't plant history. History sent to the model is what was said, without old tool results, up to 16,000 estimated tokens.
- **Sources are recorded before a tool's answer goes anywhere, then checked to still exist,** which closes the race with an erasure that scanned just before the record. A store failure there refuses the answer.
- **A run ending with nothing said is `RUN_ERROR` with code `empty_reply`,** not a quiet success. An error chunk that ADK's OpenAI-compatible client ignores was reaching the client as an empty `RUN_FINISHED`.
- **A cut reply is `RUN_FINISHED` with `result.stop: max_tokens`,** which settles spec 034's open point, and the kept message says so too.
- **At most `MEMEX_CHAT_MAX_RUNS` runs, by default 8,** then 429. One at a time per thread, then 409. With 8 runs waiting on a model: 0 active, 0 in a transaction, and 2 connections before against 4 to 5 during.
- **The AG-UI channel is unbounded,** as a full one would stall the model's stream behind a slow client, and a run's budget and tool limits already bound its size.
- **No separate cancel route.** Disconnecting is the cancel, and `HttpAgent.abortRun()` does that.
- **`POST .../chat/runs` is added by the composition root** through `chat_routes`, like MCP, so `ServerEdges` didn't change across its 33 implementations. With no model, a run gets 503.
- **`@ag-ui/core` 1.0.4 is MIT,** and its peer `zod` 3.25.76 too, both dev dependencies of `edge-agui`. `pnpm run licenses` passes, and no `deny.toml` change was needed.
- **The spike mapper's comment that the schema refuses an empty delta doesn't hold for 1.0.4,** where `delta` is any string. The mapper still drops empty ones.

**Found, for the user to know.**
- **On an OpenAI-compatible server, ADK sends the instruction as a message with role `user`, not `system`.** Seen in the request to the stand-in. So on local models it has less standing against a planted instruction than the name suggests. The mechanisms in item 2 don't rest on it, but item 1's live run on such a server would be measuring that. Gemini wasn't looked at, as that would have taken a live call.
- **The score in item 1 holds every sentence to a citation,** as it can't tell which are factual, so it errs against the agent.
- **`upsert_json_row` in `store.rs` ends in `_ => return Ok(())`,** so a new row kind with no arm there is silently not stored. This cost a debugging round here, and only a Postgres test shows it. It isn't changed, as it's outside this spec.

**Postgres tests.** The full command on a fresh database passed 42 of 42 in 17 min 19 s, at load about 25 to 110. `pg_child`, `pg_job_child`, `pg_full_disk`, `pg_kill_postgres`, `pg_bad_cases` and the `tls_` tests were not run, as the brief says. Chat adds nothing specific to any of them, though `pg_bad_cases` and `pg_kill_postgres` do exercise losing connections, which chat's short calls would see like any other store call. The coordinator runs `pg_bad_cases` and `tls_` at landing. `pnpm e2e` here is the gallery checks, and the session, shell and wiki checks weren't run, as nothing in the console changed.

**API types.** Regenerated from `memex_http::openapi()` by way of a file, because `serve` won't start without the OCR models and the brief says not to download any. It's the same function the route serves. `pnpm api:check --from` matched. The coordinator regenerates at landing.

**Follow-ups.**
- **Spec 030 must call `erase_chat_threads_citing`** after a source's rows are gone, and then compact. It should also decide what to do with `segment_ids` on `chat_run` Derivations, which are ids and no text.
- **Spec 050:** send only the newest message, or a long thread will pass the 1 MiB body limit, as `HttpAgent` sends all of them and the server reads only the last. Read `result.stop` on `RUN_FINISHED`. A cookie session needs `x-memex-csrf` on the run. The route streams `TOOL_CALL_RESULT` with the tool's whole answer, up to 64 KiB. A message has at most 32 KiB.
- **Item 1's live run,** when the user decides.
- A thread can't be exported. If it should be, that's a decision against keeping it off the log.

**Landing checks, on the merge with `main` (cce1d22), at load about 15 to 110.** The only change from `main` was the build settings in f248ac1, so the merge had nothing to conflict with. `cargo build`, `test`, `clippy`, `fmt` and `deny` passed. The Postgres command, `pg_bad_cases` on a throwaway server and the four `tls_` tests passed, and across all of these runs, `cargo test` included, 976 tests passed and 0 failed. `pg_kill_postgres_100_times` on a throwaway server: 100 cycles, highest acknowledged write 3,740, 0 lost, 0 partial, 0 gaps, 0 reopen failures, recovery p95 1.0 s. That test works on the store alone, so it says nothing about a chat run during a kill. API types were regenerated from a running server: one summary's text had drifted, and `api:check`, `typecheck` and `pnpm test` passed. Item 1's live run moved to spec 81, and the wiring is in spec 30.

**Also found, not fixed here.** `upsert_json_row` in `memex-store-postgres/src/store.rs` silently drops a row whose kind it has no arm for. A small follow-up should make that an error.
