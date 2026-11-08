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
In progress on branch `PerArneng/spec-041-ingest-live-ui`.

**Item 2's pass rule, set before anything was measured.** The console runs in Chromium against a real `memexvault serve`, through a proxy that replaces only the two activity routes with a scripted feed on the real wire format: 2,000 documents, 50 frames per second for 300 s, of which 30 are progress on documents on screen, 15 progress on documents off screen, and 5 durable, one of those a second being a frame that makes the console fetch a new snapshot of all 2,000.
- Event appearance: from the server writing the frame to the frame after the change reaches the page, for every on-screen progress frame and every durable frame. **p95 ≤ 1,000 ms.**
- Input response: from an `input` event's timestamp to the frame after it is drawn, typing in the board's filter twice a second. **p95 ≤ 100 ms.**
- Memory: the JS heap after a forced collection, once a minute. **At most 64 MiB at every sample, and the last sample at most 5 MiB above the first, taken after minute 1.**
- It passes only if all three hold over the whole 300 s. The load average is recorded beside each run. A run on a loaded machine is weak evidence either way, and a miss stays a miss.
