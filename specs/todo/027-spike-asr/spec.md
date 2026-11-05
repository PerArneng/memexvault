---
id: 027
name: spike-asr
description: Choose the meeting transcription approach by measuring word timing, audio formats and speed
---

# 027 spike-asr

## Goal
Choose how meeting recordings are decoded and transcribed. Pass: word timing error ≤300 ms median, Opus and WebM decode
without system libraries, and at least one local and one API option work behind `Transcriber`.

## Background
**Depends on:** 014. Design: `docs/architecture.md`, sections Decisions, Anchors.

Candidates: `whisper-rs` 0.16 (bundled whisper.cpp) locally and Gemini audio through the API. `symphonia` 0.6 for decoding, though
its Opus support is unverified and this codec is common in meeting recordings. Good local speaker separation may not exist, so
speaker stays optional.

## Approach
- Throwaway crate in this spec's folder.
- Corpus: public-domain or synthetic recordings only, no real meetings sent to a provider. English, Swedish, German and Spanish, multiple speakers, noise, 5 and 60
  minute lengths, and the formats WAV, MP3, M4A, Opus, WebM and MP4 video.
- Measure word error rate per language, timing error against hand-labelled points, speed relative to real time, RSS, model licence, cost per hour
  and speaker labels.
- Check that long audio doesn't lose timing accuracy when split into chunks.

## Verification
1. Results table per option and format.
2. Median word timing error ≤300 ms in each language, and WER recorded per language, for the chosen options.
3. All listed formats decode, or each gap is recorded with a workaround. External `ffmpeg` isn't an acceptable answer.
4. `otool -L` shows system libraries only for the local option.
5. A 60-minute file has no timing drift above 1 s by the end.
6. `docs/architecture.md` records the choice and removes the `memex-asr` provisional marker.

If Opus can't be decoded in-process, put the spec on hold and ask the user.

## Out of scope
Live transcription, voice agents and speaker identification by name.

## Result
Not started.
