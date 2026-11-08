---
id: 038
name: activity-stream
description: Add the per-document stage model and a live event stream for each vault
---

# 038 activity-stream

## Goal
A client can follow everything happening in a vault as it happens. Reconnecting misses nothing durable, and a slow client can't slow ingest.

## Background
**Depends on:** 014, 015, 016, 019, 024, 028, so every stage exists. Design: `docs/architecture.md`, section Live activity.

## Approach
- A pure stage model per document, derived from job and event state, with a snapshot route listing documents by stage.
- A `Progress` edge that workers report to, such as OCR page 12 of 400, held in memory. Add reporting to the OCR, transcription, embedding and extraction
  workers.
- `GET /api/v1/vaults/{id}/activity` as server-sent events. Durable frames come from the log and resume with `Last-Event-ID`. Progress frames
  have no resume.
- Frames carry ids, kinds, stages, counts and timings, never document text.
- A bounded buffer per client. Overflow sends a resync frame and drops to the current position. A pure policy decides this.
- Heartbeats, a limit on streams per principal, and an end of stream when a session or token ends.

## Verification
1. Disconnect a client at random during ingest, 200 times, resuming from its last id: 0 durable events missed or duplicated.
2. A client that never reads gets a resync or disconnect, and ingest throughput with it connected stays within 5% of no client, measured.
3. A stream for one vault never carries another vault's events, and a read-scope check runs at open and when scope changes.
4. A marker string in a document never appears in any frame, shown by scanning a recorded stream.
5. With 50 connected clients, server RSS growth is recorded and exact-lookup p95 stays within its target.
6. Bad cases: `Last-Event-ID` older than erased or nonexistent data gives a resync, not an error. Over-limit streams are refused with a clear error.
7. The stage model is unit-tested in memory, including failed, partial, waiting and retry paths.

## Out of scope
Webhooks, WebSocket transport, and push to clients outside the browser beyond this same stream.

## Result
Built on branch `PerArneng/spec-038-activity-stream`. No migration. Measured on a shared machine whose load average is printed beside each timing; at loads of 6 to 60 the timings are weak, and they're from a small vault, not 200k segments.

**What's built.**
- `GET /api/v1/vaults/{id}/activity`: a Server-Sent Events stream of the vault's event log, with progress frames from workers, `hello`, `resync`, `heartbeat` and `end`. A durable frame's id is its log position. A client resumes with `Last-Event-ID`, or `?after=` because `EventSource` can't set the header on its first request; the header wins, and -1 means before the first event.
- `GET /api/v1/vaults/{id}/activity/documents`: the board, one row a document with its stage (stored, parsing, OCR, transcribing, segmenting, indexing, embedding, extracting, review, done) and status (running, waiting, retrying, failed, partial, done), paged like every list route.
- The stage model is pure, in `memex-core/src/activity.rs`. So are the queue policy (`admit`), resume planning (`plan_resume`), and `ProgressBoard`, which keeps the latest report per job in memory and forgets finished jobs. Workers report through the `Progress` trait: rendition, OCR, transcription, embedding and extraction.
- Frames are built in `memex-api/src/activity.rs` from an explicit list of allowed fields, through an exhaustive match on `Row`, so a new kind of row fails to compile until someone decides what it may show. Ids pass `safe_id`. No text, title, file name, quote, rationale, error message or failure reason is ever copied. A failure gives only its stage and status.
- One reader task per followed vault, in `memex-http/src/activity.rs`. It polls `cache_marks` (head and generation, one statement, no transaction) every 200 ms. It holds no Postgres connection or writer lease while waiting: a statement takes a pooled connection and returns it. A ring of the last 4,096 frames serves most resumes. Older positions are read from the log, two catch-ups at a time. Each client has a bounded queue (1,024 frames or 1 MiB). When the queue overflows, the client gets a `resync` frame at the current position and carries on from there, with no gap.
- A watched connection (`conn_watch.rs`) closes when its socket has refused writes for 30 s. Only connections holding a stream are watched, so a paused download is left alone. A watched connection's kernel send buffer is fixed at 64 KiB: the kernel would otherwise grow it to 4 MiB on this machine, and a client that never reads would park that much before its own queue noticed.
- Limits: 16 streams per principal, refused with 429 `too_many_streams` and a message saying what to do. Shutdown sends `end` with `server_stopping`: `memexvault serve` exited 0.45 s after SIGINT with a stream open.
- Settings: `MEMEX_ACTIVITY_MAX_STREAMS`, `_BUFFER_FRAMES`, `_BUFFER_BYTES`, `_POLL_MS`, `_RECHECK_MS`, `_HEARTBEAT_S`, `_STALL_S`, in AGENTS.md. `docs/architecture.md` has a Live activity section.

**Verification.**
1. Passed. `pg_activity_random_disconnects_miss_and_repeat_nothing` (`cargo test -p memexvault --release --test activity -- --ignored --nocapture --test-threads=1 pg_activity`): 200 disconnects in 20.9 s during ingest, 103 of them cut at random mid-read. 1,198 documents gave 3,594 events, each seen exactly once: 0 missed, 0 duplicated. Load 6.8. The in-memory `resuming_misses_nothing_and_repeats_nothing_from_the_ring_or_the_log` covers resuming both from the ring and from the log.
2. Passed on the third run. `measure_ingest_with_a_client_that_never_reads`, real binary, 8 alternating pairs, pass rule at the top of the test file, set before measuring and unchanged.
   - Run 1, 150 documents an arm: ratios median 0.971, but 0 of 8 clients were resynced or disconnected, so it failed its own rule. The 150 documents' frames, about 400 KB, sat in the kernel's autotuned send buffer.
   - Run 2, after the 64 KiB send buffer: median 0.984, only 4 of 8 resynced. Failed. Hyper's write buffer holds up to 408 KiB (`DEFAULT_MAX_BUFFER_SIZE`) before it stops polling a body, and axum's `serve` can't lower it, so 150 documents were right at the edge.
   - Run 3, 400 documents an arm (about 1 MB of frames): ratios [1.044, 1.010, 0.968, 0.995, 0.964, 0.997, 1.005, 1.049], median 1.001 against at least 0.95. 8 of 8 resynced, 0 disconnected. About 17 documents/s per arm. Load 7 to 13.
   So a client that never reads costs at most about 408 KiB in hyper, 64 KiB in the kernel and its own queue (1 MiB) before it is resynced, and then 30 s until it is disconnected.
3. Passed. In memory, `a_stream_for_one_vault_never_carries_another_vaults_events`. Scope is checked at open and rechecked every second by the reader, per distinct credential, not only at open. On Postgres through HTTP (`pg_activity_scope_changes_end_streams`), the time from the change to the stream's `end`:

   | Change | Time to `end` |
   |---|---|
   | Token revoked | 1,027 ms |
   | Session ended | 24 ms |
   | Principal disabled | 964 ms |
   | Grant removed | 979 ms |
   | Token expired | 997 ms |
   | Vault deleted | 48 ms |

   Frames can still flow for up to one recheck (1 s) after a revocation. If the store can't answer a recheck, the stream continues for up to 10 s and then ends with `unverifiable`.
4. Passed. In memory, `a_marker_in_a_document_never_appears_in_any_frame` plants the marker in a document, page, entity, claim, evidence quote, rationale, extraction and audit failure reason, a recording's transcript, and error messages, then scans every byte of a recorded stream. Every table on `main` reaches a frame except `AutoCommit` and `AuditPlanted`. Those two are count-only by the exhaustive match and carry no fields. On Postgres (`pg_activity_marker_scan`): 45 events, 23 holding the marker, all 9 kinds that test makes. 22,972 bytes replayed and 23,160 live, with no marker in either.
5. **Failed** on the absolute target. `measure_fifty_clients`, real binary, 6 alternating pairs of 2,000 exact lookups over HTTP.
   - p95 with 50 clients [3.15, 3.11, 3.24, 3.05, 2.89, 3.02], median 3.08 ms, against at most 2 ms.
   - p95 without clients [2.90, 2.72, 3.16, 2.97, 3.12, 2.95], median 2.96 ms, so the lookup already misses the target with no stream open. The streams add a ratio of 1.027.
   - RSS grew at most 2,064 kB in the run, 41 kB a client. macOS compressed the server's idle pages during the last pair, so RSS fell from 504 MB to 35 MB, and RSS here is a rough measure.
   - Postgres connections: 9 idle, peak 9 with 50 clients and peak 9 without, from `pg_stat_activity`. The streams added 0.
   - Load 6.4 to 7.7. The spec 020 baseline also missed 2 ms, and a lookup over HTTP adds statements to authenticate.
6. Passed. `pg_activity_a_position_the_log_no_longer_holds_gets_a_resync` and in-memory `a_bad_position_gets_a_resync_and_not_an_error`: a position past the head, one the log no longer holds, and nonsense each give `resync`, not an error. Bumping the cache generation resynced the stream after 1 frame. A second stream over a limit of 1 gave 429 with "this principal already has 1 activity streams open, which is the limit, so close one first".
   Erasure is spec 030 and isn't built, so "older than erased" is tested only by simulating a log that no longer holds the position, through a generation bump and missing rows. Spec 030 has to call `bump_cache_generation` when it erases. That empties the ring and resyncs every stream. It may then keep or remove the event envelopes: an emptied payload gives a frame with no detail, and missing rows give a resync.
7. Passed. 21 unit tests in `memex-core/src/activity.rs` cover the stage model, including `retry_waiting_and_failed_paths_are_visible`, `a_failed_page_or_a_skipped_stretch_makes_a_document_partial`, `partial_is_visible_at_the_end_and_while_review_waits`, `an_earlier_failure_is_not_hidden_by_later_work`, queue admission, resume planning and the board's bounds. 9 more are in `memex-api/src/activity.rs`.

**Checks.**
- `cargo build --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check` and `cargo deny check licenses` are clean. `cargo test --workspace`: 908 passed, 0 failed, 99 ignored.
- The Postgres suite on a fresh database, `cargo test -p memexvault --release --no-fail-fast -- --ignored --test-threads=1 pg_` with `pg_child`, `pg_job_child`, `pg_full_disk`, `pg_kill_postgres`, `pg_bad_cases` and `tls_` skipped: 42 passed, 0 failed. Those six were not run. The stream holds no connection or lease between polls, which is what they would stress, and it adds no migration or TLS path.
- Console: `pnpm typecheck`, `lint`, `test` (191), `run licenses` and `run contrast` pass. `pnpm e2e` failed once in `gallery.spec.ts` (keyboard focus) at load 14.9 and passed 6 of 6 when rerun. That test doesn't touch the API types, which are the only console change.

**Decisions.**
- `ProgressBoard` is held directly on `Server`, not as a `ServerEdges` type. It does no IO, and an edge type would have touched 34 implementations.
- The stream has its own SSE code, separate from spec 049's chat stream, as the brief asked.
- `?after=` exists beside `Last-Event-ID` for `EventSource`.
- A watched connection's send buffer is 64 KiB.
- Polling costs 5 statements a second per followed vault, and about 3 a second per distinct credential for the recheck. Catch-up reads whole events pages, with at most 2 running at once.
- The indexing stage compares times, so it can show a document past indexing slightly early.

**Follow-ups.**
- Any test that sends a request to every route and reads each body to the end now meets a stream that never ends. `session.rs`'s cookie walk hung on it until it read only the status.
- `web/packages/api/openapi.json` was generated on this branch, so regenerate it after merging `main`'s chat routes.
- Lowering hyper's write buffer for streams needs serving through hyper-util's builder instead of `axum::serve`.

**Landing checks, on the merge with `main` with specs 049 and 042 in it (ae7cd4a), at load about 5 to 24.** Seven files conflicted, all shared lists, the server's fields and the listener. Both sides were kept. The listener is this spec's watcher, which sets `TCP_NODELAY` on every connection as spec 049's chat stream needs. The list-route count in `tests/api.rs` is 29. The frame code's lack of a catch-all did its job: spec 049's `ChatSettings` row didn't compile until it was listed, and it's count-only in frames, like the audit rows. Chat threads and messages aren't on the log, and a proposal from chat is an ordinary changeset, so the allow-list already covers both. The marker fixture doesn't run a chat, so item 4's scan didn't exercise that path. `cargo build`, `test`, `clippy`, `fmt` and `deny` passed. The Postgres command, `pg_bad_cases` on a throwaway server and the `tls_` tests passed, and across all of these runs, `cargo test` included, 1,032 tests passed and 0 failed. `pg_kill_postgres_100_times` on a throwaway server: 100 cycles, highest acknowledged write 6,417, 0 lost, 0 partial, 0 gaps, 0 reopen failures, recovery p95 0.5 s, on the store alone. The API types regenerated from a running server matched the committed ones. Items 2 and 5 weren't measured again on the merge, as nothing merged in touches ingest, streams or exact lookup.

**The user's decision at landing.** Spec 038 landed with item 5's 2 ms target missed. Exact lookup over HTTP misses it with no stream open, and 50 streams add about 3%. Spec 062 holds it with the other misses of that target.
