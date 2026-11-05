---
id: 028
name: meeting-ingest
description: Ingest meeting recordings so claims cite exact playable time ranges in the original audio
---

# 028 meeting-ingest

## Goal
Meeting recordings go through the same pipeline as documents. Every claim from a recording cites a time range in the
original file that can be played back.

## Background
**Depends on:** 017, 024, 027. Design: `docs/architecture.md`, section Anchors.

## Approach
- Upload accepts audio and video, with the original kept unchanged as the Source blob.
- `Transcriber` with the chosen local and API implementations, selected by settings. API use is off unless configured,
  because recordings are often sensitive.
- A Rendition with a timed word table. Segments follow speaker turns and pauses.
- Anchors use `t_start_ms` and `t_end_ms` on the original timeline. The extraction agent is reused unchanged, with quotes
  located to word times.
- The provenance API returns a time range, and a byte-range stream of the original file.
- Meeting metadata: title, date and participants, with participants linkable to Person entities through proposals.

## Verification
1. On a labelled recording, for at least 50 claims, the cited range contains the spoken quote at least 95% of the time, with the
   start within 1 s.
2. 100% of committed claims from recordings have a resolving time anchor.
3. A search can return meeting evidence and document evidence in one package with correct citations.
4. The transcript API can return text around a time, with speakers where known.
5. A Range request plays from the cited time in a test player, or is byte-verified where a player is impractical.
6. Sending to an API transcriber is impossible unless enabled, proven by a test with it disabled.
7. Bad cases: silence, no speech, corrupted audio and very long files stay within memory limits and fail one job only.

## Out of scope
Live capture, calendar integration and meeting summaries as a separate feature.

## Result
Not started.
