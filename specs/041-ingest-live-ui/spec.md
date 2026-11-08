---
id: 041
name: ingest-live-ui
description: Build uploads and the live view of document processing
---

# 041 ingest-live-ui

## Goal
A user uploads documents and recordings and watches them move through the pipeline live, down to OCR page 12 of 400, and can act on failures. It stays usable at 50 events per second, and an event's appearance has p95 ≤1 s.

## Background
**Depends on:** 038, 040. Design: `docs/architecture.md`, section Live activity, and `docs/frontend-architecture.md`, section Live data.

## Approach
- Drag-and-drop upload with per-file progress, a queue with limits from the published API, trust tier and metadata, and clear duplicate handling.
- A live board by document and stage, with counts per stage, throughput, and time in stage.
- Document detail: a timeline of stages, attempts, errors, and retry and cancel.
- Dead-letter list and bulk retry. A "waiting for LLM" state explained in plain words.
- `SseActivityFeed`, a pure reducer merging snapshot and events, batched rendering and virtualized lists. Resync frames fetch a new snapshot, and a connection indicator shows the stream's state.

## Verification
1. A scripted run of a digital PDF, a scanned PDF and a recording shows every stage in order in Playwright, against the real backend.
2. A load test at 50 events/s for 5 minutes: event appearance p95 ≤1 s, input response ≤100 ms, and no memory growth beyond a recorded bound.
3. Killing the connection mid-run recovers to the correct state with no reload, and durable counts match the API.
4. A failed job can be retried from the UI, and cancel stops a running job. Both are attributed to the signed-in principal.
5. The reducer is unit-tested with out-of-order, duplicate, resync and unknown-kind events, using `ManualClock`.
6. Bad cases: an oversized file, a disallowed type, a lost connection during upload, and a read-scope user, who sees the view with no upload controls.

## Out of scope
Resumable uploads, folder sync and source connectors.

## Result
Built on branch `PerArneng/spec-041-ingest-live-ui`. No migration and no change under `crates/`. Measured in Chromium, the only browser set up, on a debug backend, on a shared machine whose load average is beside each number. At these loads the timings are weak evidence, and they're also from the debug build's server, though the times that count are the browser's.

**What's built.** The Ingest section, as a lazy chunk of 25.41 kB (7.77 kB gzip); the sign-in route is 128,507 bytes gzip against the 200,000 limit.
- Upload: drag and drop or a file button, per-file progress, a queue of 2 at once, limits from `/upload-limits`, trust tier, a recording's language, and a duplicate named as such with a link to what's already there.
- Board: counts per stage, finished per minute, events per second, a connection indicator, filters kept in the URL, and a virtualized table with progress such as "OCR page 12 of 400" and time in stage.
- Document view: a timeline of every stage, attempts with their errors, retry and cancel confirmed in the page, and "waiting for the LLM" explained in plain words.
- Failed jobs at `ingest/-/failed`: dead and cancelled, with selection and bulk retry.
- `SseActivityFeed`, `activityReducer`, `ActivityWatch`, `createUploadQueue` and `createXhrUploader`. `docs/frontend-architecture.md` says how they fit together.

**Verification.**
1. **Passed, with a limit.** `e2e:ingest`, a digital PDF, a scanned PDF and a recording uploaded through the UI to a real `memexvault serve`, 17.1 s at load 77 to 141. The board never moved a document backwards, and each showed extraction, review and done live. Each timeline names every stage in order. The limit: a stage that lasts less than the gap between two snapshots can pass unseen. In the final run segmenting and embedding were unseen on both PDFs, and the recording saw all of its stages; an earlier run saw all of them on all three. The timeline marks such a stage "finished, not seen live" rather than leaving it out. Embedding is the one frames can't show, as an embedding job isn't tied to one document. The LLM was a stand-in server on this machine, so nothing called a provider and this says nothing about a model. Rendition jobs were kept short because of spec 82.
2. **Passed** against the rule below, set before measuring. 303 s, 15,045 frames, 49.6 per second, 2,000 documents, 1 stream, 252 snapshots of all 2,000 (2,520 pages). Appearance over 10,530 frames with 0 missing: p50 101 ms, **p95 196 ms**, p99 391 ms, max 975 ms. Input over 594 keystrokes: p50 4 ms, **p95 17 ms**, max 21 ms. Heap after collection, minutes 1 to 5: 8.31, 8.63, 8.75, 8.70, 8.98 MB, so **0.68 MB growth** and a peak far under 64 MiB. Load fell from 70 to 37 during it. An earlier run at load 88 to 257 gave p95 196 ms, input p95 16 ms and 0.70 MB growth; that run's one missing frame was the harness still writing after its last look, which was fixed by stopping first. The feed is scripted, as the spec allows, through a proxy in front of the real server; it doesn't measure the server's own stream, which spec 038 did.
3. **Passed.** Three kills of every connection mid-run, through a proxy, with the server and Postgres untouched, and new connections refused for 4 s while another document was uploaded past the cut. The indicator showed reconnecting after 93 to 194 ms and live again after 7.6 to 8.0 s. Each stream resumed from the position it had reached (76 gave 77, 144, 203). No navigation happened, and a value set on `window` before the first kill survived. On 22 documents, screen and API both said done 22. Load 77 to 96.
4. **Passed, with a limit.** From the UI, a running OCR job on a 6-page scan was cancelled, stayed cancelled, and after 20 s no rendition had landed; retried, it finished. A dead job (a false PDF) was retried from its page and again from the bulk list. The log has the signed-in principal as actor on all four changes, and the page says "Cancelled by" or "Retried by" that name. The limits: no route shows an event's actor, so the check reads the log with `psql` on this run's database, and a reload loses the page's line. And cancel doesn't interrupt the worker: what it's computing carries on to the end of that attempt and is then refused, by the fence that already exists. Load 101.
5. **Passed.** `activity.test.ts` and `activity-watch.test.ts`, on `ManualClock` with no sleeping: out-of-order, duplicate, gap, resync, unknown frame, unknown event kind and unknown stage, plus replay during a snapshot, the caps and each connection state.
6. **Passed.** An oversized file (a sparse 3 GB file, refused from its size with no request sent), a disallowed type (`.zip` refused at once, and a binary `.bin` refused by the server with its answer in words), a lost connection during upload (cut at 2.4 MB of 10 MB, nothing stored, and the same file retried), and a read-scope user who sees the board, a document and the failed list with no upload, retry, cancel or select controls. The server's own 403 for that user is checked too. Axe found 0 violations on the board and the document view. No modal was added, so no new CSP error: every check above asserts 0.

**Decisions.**
- Ingest's section needs `read`, not `propose`, as item 6 asks. That changes spec 040's rule and its tests in `navigation.test.ts`, `commands.test.ts` and `shell.spec.ts`.
- The stage model stays on the server. A frame moves a document only up to the end of parsing, and anything else asks for a snapshot, at most once a second and at most a fifth of the time. The first version moved a document into extraction from its job's frame, and item 1 caught it going backwards, as embedding and extraction overlap.
- `fetch`, not `EventSource`, to see a refusal's status and notice a silent connection: 45 s with no bytes reconnects.
- Upload queues live in the engine per vault, so moving to another screen doesn't stop an upload. Signing out aborts them. A page reload still loses what's in flight.
- `Edges.activity` and `Edges.uploader` are optional, so other screens' tests needn't change.
- A timeline after a reload takes recording and OCR from the source's type and its rendition's pages. Time in stage restarts at a reload, as the snapshot doesn't say when a stage began.
- The only metadata offered is a recording's language, which is all the upload route takes besides trust tier. `replaces` isn't offered, as Documents has its own idea of versions.

**Other screens' checks, on this branch.** `e2e:shell` 17 of 17, with the read token's expected navigation now starting at Ingest. `e2e:wiki` 17 of 17. `e2e:documents` 9 of 11 with no speech in the recording: item 3 needs speech, and passed on a rerun with the first 600 KB of `en_scotus.mp3`; item 5 missed at a median of about 1,800 ms at load about 17, on the debug build. Spec 042 records that same miss on debug, at 2,308 ms, and its pass on release, and this branch changes nothing the viewer uses, so that's not a regression, but release wasn't run here.

**Not run.** `cargo test --workspace`, `cargo clippy` and the Postgres command, as nothing under `crates/` changed. That includes `pg_child`, `pg_job_child`, `pg_full_disk`, `pg_kill_postgres`, `pg_bad_cases` and `tls_`, none of which this affects.

**Follow-ups.** No route shows an event's actor, so attribution can't be displayed after a reload: the user decides whether that's worth a route. Until spec 82 lands, a large document holds both rendition workers: two 10 MB text fixtures here kept requeueing past their 60 s lease across server restarts and stalled every later upload for over 5 minutes, which is why the checks now use a false PDF and cancel what they leave. `PgStore`'s unbounded connections showed up too: with other workers running, 72 connections were in use, a server failed to start, and vault creation got 503s, so `e2e:ingest` waits those out in its own calls. The board was measured at 2,000 documents and holds up to 10,000, where sorting costs about five times as much.

**Item 2's pass rule, set before anything was measured.** The console runs in Chromium against a real `memexvault serve`, through a proxy that replaces only the two activity routes with a scripted feed on the real wire format: 2,000 documents, 50 frames per second for 300 s, of which 30 are progress on documents on screen, 15 progress on documents off screen, and 5 durable, one of those a second being a frame that makes the console fetch a new snapshot of all 2,000.
- Event appearance: from the server writing the frame to the frame after the change reaches the page, for every on-screen progress frame and every durable frame. **p95 ≤ 1,000 ms.**
- Input response: from an `input` event's timestamp to the frame after it is drawn, typing in the board's filter twice a second. **p95 ≤ 100 ms.**
- Memory: the JS heap after a forced collection, once a minute. **At most 64 MiB at every sample, and the last sample at most 5 MiB above the first, taken after minute 1.**
- It passes only if all three hold over the whole 300 s. The load average is recorded beside each run. A run on a loaded machine is weak evidence either way, and a miss stays a miss.
