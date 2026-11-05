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
Not started.
