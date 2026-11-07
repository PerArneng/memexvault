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
- Routes for meeting metadata read and write, full transcript, and media type and duration.

## Verification
1. On a labelled recording, for at least 50 claims, the cited range contains the spoken quote at least 95% of the time, with the
   start within 1 s.
2. 100% of committed claims from recordings have a resolving time anchor.
3. A search can return meeting evidence and document evidence in one package with correct citations.
4. The transcript API can return text around a time, with speakers where known.
5. A Range request plays from the cited time in a test player, or is byte-verified where a player is impractical.
6. Sending to an API transcriber is impossible unless enabled, proven by a test with it disabled.
7. Bad cases: silence, no speech, corrupted audio and very long files stay within memory limits and fail one job only.
8. The full transcript's time ranges agree with the provenance API's ranges on a fixture recording.

## Out of scope
Live capture, calendar integration and meeting summaries as a separate feature.

## Result
**Item 1's rule, set before anything was measured.**
- **Recording:** `en_5m.wav` from spec 027's corpus. It's synthetic speech with four voices, and its truth comes from the synthesizer's word markers, not from hand labels. The other three languages' `*_5m.wav` run the same way and are reported beside it, and they aren't what the rule decides on. If `en_5m.wav` gives under 50 sentence claims, `en_60m.wav` decides under the same rule, and the shortfall is reported.
- **Claims:** the extraction job runs, with a scripted agent and no LLM, so this says nothing about a model's choice of quotes. It proposes one claim per sentence of every segment the real transcript gave, with no sentence left out, keeping sentences of at least 3 words. A person's approval commits them, and each gets its anchor as it commits, as usual.
- **Alignment:** transcript words are aligned to truth words by a diff on lowercased alphanumerics. A quote's truth range runs from the truth start of its first aligned word to the truth end of its last aligned word.
- **A claim passes** when its cited `t_start_ms` is at or before that truth start, its `t_end_ms` is at or after that truth end, and `t_start_ms` is within 1,000 ms of the truth start. A claim with under half of its words aligned has no truth to check against, and counts as a failure.
- **The item passes** with at least 50 claims and at least 95% of them passing. Padding is 300 ms on each side, set from spec 027's finding that starts run 150-210 ms late, and it isn't tuned on this measurement. If it fails, the number stays failed.

**Item 1's first result: failed.** Under that rule, with 300 ms of padding, on `en_5m.wav`: 60 claims, 54 pass, 90.0% against 95%. All 6 failures start after the truth, by 4 to 255 ms, and none ends early or starts over 1 s away. 51 of the 60 had every word aligned. This number stands, and nothing below replaces it.

**Item 1's second rule, set after that result and before any other recording was measured.**
- **The change:** starts get 600 ms of padding, and ends keep 300 ms. The 600 comes from `en_5m.wav` alone: its worst miss needed 555 ms. So `en_5m.wav` can't judge it, and it isn't rerun for the record.
- **Recording that decides:** `en_60m.wav`, which nothing has been measured on in this spec. It's the same corpus: synthetic voices, truth from the synthesizer's markers, and not hand labels, with noise at 20 dB and long lulls. `sv_5m`, `de_5m` and `es_5m`, also unseen, are reported beside it and don't decide.
- **Everything else is as before:** the scripted agent, one claim per sentence with none left out, the alignment, and what counts as a pass, at 95% of at least 50 claims.
- **If it fails,** that's reported as failed as well, and the padding isn't changed again here.

Not finished.
