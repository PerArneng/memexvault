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
Not started.
