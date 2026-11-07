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

**Item 1's second result: failed.** Under the second rule, on `en_60m.wav`: 708 claims, 657 pass, 92.8% against 95%. The failures are 34 starts over 1 s before the truth, 4 ends before the truth and 15 claims with under half their words aligned, and two claims fall in two of those counts. No start is late, so the padding fixed what it was meant to fix and moved the miss to the other side. The labels are the synthesizer's word markers, not hand-made, the agent is scripted, and no model is live. The padding stays at 600 ms, as the rule said, so `asr_item_1_on_an_hour` fails, and it should until someone sets a new rule.

Of the 8 over-1 s failures the test printed, 5 are off by 1,001 to 1,202 ms. Several start with a misheard word, such as "Rukasal" or "Oakeshit". The truth range starts at the first aligned word, so those claims can start early because of the alignment and not the audio. That's an observation from 8 samples, and the claims weren't re-scored. The rule counts them as failures, and so does this result.

Reported beside it, not deciding:

| Recording | Claims | Pass | Late starts | Ends early | Over 1 s | Under half aligned | Median lead |
|---|---|---:|---:|---:|---:|---:|---:|
| `en_60m` (decides) | 708 | 92.8% | 0 | 4 | 34 | 15 | 468 ms |
| `sv_5m` | 68 | 92.6% | 0 | 1 | 3 | 1 | 451 ms |
| `de_5m` | 58 | 98.3% | 0 | 0 | 0 | 1 | 432 ms |
| `es_5m` | 70 | 100.0% | 0 | 0 | 0 | 0 | 474 ms |

**Verification.** Timings are from a busy shared machine, with a load average of about 600 to 650. Every item that needs real audio ran on synthetic speech.
1. **Failed**, as above. Command: `cargo test -p memexvault --release --test meeting_eval -- --ignored --nocapture --test-threads=1 asr_item_1_on_an_hour`, with the variables `AGENTS.md` lists.
2. **Passed:** 0 of 708 claims on `en_60m` lack a resolving time anchor, and 0 of 68, 58 and 70 on the other languages. `pg_meeting_every_committed_claim_has_a_time_anchor_that_resolves` also gives 0 of 60 on a word table made in the test.
3. **Passed on a word table made in the test, not on real audio:** `pg_meeting_a_search_returns_meeting_and_document_evidence_in_one_package` returns 4 items, 4 citations and 2 sources in one package.
4. **Passed:** the HTTP tests in `crates/memex-http/tests/meeting.rs` return text around a time, with speakers where known, and it's the same function that item 8 checks on real audio.
5. **Passed, byte-verified:** on each of the 4 recordings, 10 cited ranges fetched by Range were identical to the original's bytes, and a fresh transcriber heard at least 80% of each quote's words in all 40, averaging 98% to 99%. This isn't a browser player. It works out the byte from a WAV header read by Range, as a player would. The HTTP test covers ranges, 416 and streaming.
6. **Passed, and the live run was NOT RUN.** With the API's key, model and address all set and the API not named, 0 requests and 0 bytes reached a fake provider, and the parser string names only the local models (`asr_the_api_is_off_unless_named_and_then_runs_on_the_local_clock`). Named, against the fake, it sent 1 request of 12,801,471 bytes and gave 811 words on the local clock, with a median start error of 178 ms against the fake's own 30,000 ms. The fake says nothing about the provider.
7. **Passed:** see below.
8. **Passed:** 0 of 708 ranges differ between the transcript and provenance on `en_60m`, and 0 of 68, 58 and 70 on the other languages.

**Bad cases (item 7), with a memory limit of 4,096 MB:**
- **Long:** 7 h of silence is refused in 2 s at 12 MB. 4 h of silence takes 500 s at a peak of 79 MB and gives 0 words.
- **No speech:** 1 min of noise peaks at 50 MB and gives 0 words.
- **Speech:** 60 min through the worker alone takes 249 s (14.5× real time) at a peak of 870 MB, with 9,449 words, the last word off by 405 ms, and no stretch skipped or made coarse. The end-to-end test took 832 s for the same audio.
- **Corrupted, 15 files from spec 027:** 7 finish, 5 go dead with a stated reason, and 3 are refused at upload. Silence finishes with 0 segments. Bit-flipped `.mp3` and `.opus`, and truncated `.mp3`, `.opus`, `.wav` and live `.webm`, finish with what could be read. Bit-flipped `.m4a` and `.webm`, truncated `.mp4` and `.webm`, and an `.mp4` with no audio go dead.
- **One job only:** `pg_meeting_a_job_that_failed_leaves_others_going` passes.
- **A killed worker:** killed after 3 stretches, it carried on from its progress and gave 809 words, the count of an unbroken run, with 1 coarse stretch, a peak of 782 MB and a median start error of 203 ms.

**Also measured:** on `en_5m` in 6 containers and 3 files with late-starting audio, the median word start error is 169 to 181 ms, with 809 to 825 words and a peak of at most 797 MB. The word error is before padding. On `en_60m`, 94.1% of words align to truth, with a median word start error of 195 ms.

**Regression:** the Postgres command from the brief, on a freshly recreated `memex_s028`, passes 30 of 30 with no `LeaseLost`. These weren't run: `pg_child`, `pg_job_child`, `pg_full_disk_returns_an_error_and_data_stays_readable`, `pg_kill_postgres_100_times`, `pg_bad_cases` and the `tls_` tests. This branch doesn't change `memex-store-postgres` or add a migration, so I don't expect it to affect what they cover. The new fields are optional inside `row jsonb`, and that hasn't been tested by those six.

**The API's live run: NOT RUN.** It calls the provider, costs money and sends audio to it. The command, on a folder holding the recording in a file named `source`:

`MEMEX_BACKEND_TRANSCRIBER=gemini MEMEX_TRANSCRIBE_API_MODEL=gemini-3.8-flash MEMEX_TRANSCRIBE_API_KEY=<key> MEMEX_TRANSCRIBE_API_MAX_TOKENS=<cap> MEMEX_MODELS_DIR=<dir> target/release/memexvault transcribe-worker <folder> en`

Spec 027's estimate is 91,000 to 113,000 input and 354,000 to 438,000 output tokens per hour of audio, about $1.41 to $1.71 per hour now and $2.82 to $3.42 from 2027-01-01.

**What was built:**
- **`memex-asr`:** decoding with `symphonia` and `opus-rs` onto one timeline, covering MP4 edit lists, Ogg pre-skip, holes as silence, and a refusal above 1% bad packets. Also the local transcriber, `whisper-rs` with Silero VAD and DTW word starts in a child worker, and the Gemini API retimed to the local clock.
- **`memex-core`:** the pure `meeting` module and its `Transcriber` trait and contract, `play_range` as the one place ranges are made, time on anchors and citations, recording sniffing, and meeting proposals.
- **The job:** `transcribe_job` is its own job kind and pool of 1, at `nice` 10, with a lease, RSS and stall limits and saved progress.
- **HTTP:** `meeting`, participant `link`, `transcript`, `media` and a streaming `stream` with Range, plus regenerated API types.
- **Models:** `memexvault models fetch` also fills `whisper/`.

**Decisions:**
- Spec 27's choice stands, as spec 61's assumption says: local first, and the API off unless `MEMEX_BACKEND_TRANSCRIBER=gemini`. With it off, the worker doesn't get the key at all.
- Meeting metadata lives in `Source.metadata`, through proposals. Participant links are Edges with deterministic ids.
- The word table keeps the model's times, and padding happens at read, so changing it needs no re-transcription.
- The language hint comes from the upload's `language`, and otherwise whisper detects it.
- An API reply matching under 50% of the local words fails the job rather than storing doubtful times. A stretch that crashes the worker twice is skipped and counted, and 8 crashes end the job.
- Migrations: none, as `row jsonb` holds the new fields.

**To approve or decide before landing:**
- `Unlicense` in `deny.toml`, for `whisper-rs` and `whisper-rs-sys`, was added as the brief directed, and the user hasn't approved it. `symphonia` is MPL-2.0, which `deny.toml` already allowed, and `cargo deny` refused nothing else.
- Item 1 is missed on both rules. A third attempt needs a new rule and a recording nothing has been measured on, and hand labels on real speech would say more than these.

**Follow-ups:**
- A console screen for recordings.
- Erasure (spec 030) must also clear `<vault>/transcribe/<job>` folders.
- Speaker labels aren't matched across API chunks.
- After a revert, re-linking the same participant with the same idempotency key replays the old changeset, so a client must send a new key.
- `/file` still reads a whole file into memory, while `/stream` doesn't.
