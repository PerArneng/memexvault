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
**Decision, kept. Local: `whisper-rs` =0.16.0 with `ggml-large-v3-turbo-q5_0` (574 MB), DTW word timing and Silero VAD. Decoding: `symphonia` =0.6.1 plus `opus-rs` =0.1.41. API: `gemini-3.8-flash` for text and speakers, placed on the local model's clock.** Spec 061 has the choices for the user to review.

Gemini's own word times aren't usable. So the API option passes only with the local model running too: it improves the text and adds speakers, but it doesn't remove the local model. It's also the only API tested, because Gemini is the only key available.

Results on the 5-minute WAV, unless a row says otherwise. Start is the median word start error against synthetic truth. All runs are on the M1 Max:

| Option | WER en/sv/de/es % | Start en/sv/de/es ms | Words off by >1 s | Largest drift | Speed vs real time | Peak RSS | Cost per hour |
|---|---|---|---|---|---|---|---|
| **Local, chosen** | 2.3 / 10.0 / 7.0 / 3.2 | 177 / 155 / 200 / 156 | 0% | 219 ms | 15-17× Metal | 0.9 GB | none |
| Local, noise at 10 dB | 6.8 / 13.5 / 12.3 / 7.7 | 192 / 164 / 208 / 172 | 0% | 233 ms | 14-18× | 0.9 GB | none |
| Local, 60 min | 5.6 / 15.1 / 12.7 / 7.7 | 194 / 164 / 212 / 167 | 0-0.3% | 224 ms | 17× | 1.3 GB | none |
| **API on local clock, chosen** | 2.2 / 4.3 / 4.1 / 1.8 | 178 / 157 / 200 / 158 | 0-0.3% | 218 ms | 1.1× | 0.8 GB | $1.41-1.71 |
| API on local clock, noise | 5.9 / 6.0 / 6.3 / 3.3 | 194 / 166 / 207 / 173 | 0-0.5% | 230 ms | | | |
| API on local clock, 60 min (en, sv) | 4.3 / 7.6 / - / - | 196 / 167 / - / - | 0.2-0.3% | 205 ms | | | |
| Gemini's own times | 2.2 / 4.3 / 4.1 / 1.8 | 300 / 238 / 205 / **1608** | 0-61% | **3.6 s** | 0.6-1.9× | 0.09 GB | same |
| Gemini's own times, 60 min (en, sv) | 4.3 / 7.6 / - / - | 303 / 358 / - / - | 12-21% | **39.6 s, 79.8 s** | 1.4-1.5× | 0.5 GB | same |
| Local without VAD, 60 min | 16.5 / 19.6 / 23.5 / 11.7 | 196 / 163 / 211 / 165 | 0.1-0.7% | 224 ms | 15-17× | 1.6 GB | none |
| Local without DTW | 2.1 / 12.1 / 6.0 / 4.3 | **363 / 349 / 422 / 373** | 5-12% | 546 ms | 16-20× | 0.8 GB | none |

The API-on-local-clock rows were built offline from saved runs by `tools/hybrid.py`. The Rust `Retimed` transcriber then ran end to end on Spanish, Gemini's worst timing file: WER 1.2%, start 158 ms, 1.1× real time. Those are the speed and RSS in the table.

Local per format, WER % / start ms (`vad`, `vad-formats` in `results.txt`):

| Format | en | sv | de | es |
|---|---|---|---|---|
| WAV | 2.3 / 177 | 10.0 / 155 | 7.0 / 200 | 3.2 / 156 |
| MP3 | 2.3 / 175 | 9.4 / 154 | 6.6 / 196 | 3.2 / 155 |
| M4A | 7.4 / 178 | 10.2 / 156 | 7.1 / 201 | 3.5 / 156 |
| Opus | 3.6 / 176 | 9.7 / 154 | 7.0 / 199 | 3.2 / 157 |
| WebM | 3.6 / 176 | 10.0 / 156 | 7.0 / 198 | 3.2 / 157 |
| MP4 video | 3.6 / 176 | 10.2 / 154 | 7.1 / 200 | 3.0 / 156 |
| WebM, audio 1.5 s late | 3.2 / 169 | 10.2 / 150 | 6.3 / 192 | 3.3 / 152 |

English M4A's 7.4% is one stretch of inserted and dropped words, not a decode problem: whisper's output changes with tiny input differences. The API wasn't run per format, because it receives the same decoded PCM, and the decoded timelines differ by at most 5.6 ms (`decode_results.txt`).

Real audio, where only text has a reference (`corpus.md`):

| File | Local WER | Gemini WER | Local word starts inside a pause | Gemini word starts inside a pause | Gemini vs local start, median | Gemini vs local, >1 s apart |
|---|---|---|---|---|---|---|
| English, court, 70 min (Gemini: first 10) | 7.1% | 3.6% | 0.0% | 3.3% | 380 ms | 19% |
| Swedish, 14.5 min (Gemini: 10) | 11.0% | 8.0% | 0.0% | 2.6% | 980 ms | 49% |
| German, 41 min (Gemini: 10) | 2.7% | 0.7% | 0.2% | 1.6% | 1325 ms | 51% |
| Spanish, 7 min | 10.8% | 7.4% | 0.0% | 0.5% | 1050 ms | 52% |
| Spanish, 27.8 min | 8.2% | not run | 0.1% | | | |

Taking the local model as the clock, because its starts almost never land inside a pause, Gemini's times are worse on real audio than on synthetic audio. Most of the local English error is 735 dropped words out of about 13,000, which isn't explained yet. Local speed on real audio is 14.5-18×, with RSS up to 1.7 GB.

Other measurements:
- **Models compared, 5-minute WAV without VAD, WER en/sv/de/es:** `base-q5` 9.7 / 35.5 / 20.9 / 24.5, `small-q5` 4.7 / 34.5 / 10.5 / 8.8, `turbo-q5` 2.1 / 12.1 / 6.0 / 4.3, full `turbo` 1.8 / 11.1 / 6.3 / 3.4 at 1.9 GB. Timing barely differs between models. Beam 5 is no better, and on English it's worse.
- **CPU only:** 4.4-5.2× real time at 4 threads and 5.5-6.6× at 8, with the same accuracy and 1.4 GB. Linux wasn't tested.
- **Chunking:** whisper loses no timing. Whole file, 300 s chunks and VAD spans all stay within 226 ms of drift. Gemini's errors are inside its chunks, usually a stretched timeline, and don't build up across them. 60 s chunks didn't help (335 ms on English).
- **Other Gemini models, English:** `3.5-flash-lite` 91 s and `3.6-flash` 80 s median error, `3.5-flash` 283 ms with 4% of words over 1 s, and `3.5-transcribe` returns text with no times.
- **Speakers:** local has none. Gemini's labels are 87-100% pure on 5-minute English, German and Spanish, and it finds all 4 speakers. Labels restart each chunk (52 labels for a 4-speaker hour), so linking them across chunks is unsolved. Swedish's 54% isn't a fair test, as its speakers are one voice.
- **Cost:** 91,000-113,000 input and 354,000-438,000 output tokens per hour. That's $1.41-1.71 per hour now and $2.82-3.42 from 2027-01-01, at the prices on aistudio.google.com/docs/pricing read on 2026-11-06. This includes retries. Swedish needed 2-3 attempts on some chunks because of unparseable replies. Free-tier content can be used to improve Google's products, so real recordings need the paid tier.
- **Licences:** Whisper weights and whisper.cpp MIT, Silero VAD MIT, `opus-rs` BSD-3, `symphonia` MPL-2.0, `whisper-rs` and `whisper-rs-sys` Unlicense.
- **Model files:** sha256 prefixes `394221709cd5ad1f` (`ggml-large-v3-turbo-q5_0.bin`, `ggerganov/whisper.cpp` on Hugging Face), `2aa269b785eeb53a` (`ggml-silero-v6.2.0.bin`, 0.9 MB, `ggml-org/whisper-vad`).
- **Opus decoders, 5 minutes against libopus:** `opus-rs` 439-719× at 78-81 dB. `opus1` is faster but has 312 downloads. `unsafe-libopus` scored -11 dB on one file. `opus-decoder` manages 9-17×. `opus-rs` had no panics on 400,000 mutated packets, and is 0.1.x with 251 `unsafe` mentions, so it's pinned.

Findings for spec 028:
- **Whisper can abort the process.** A DTW assert in whisper.cpp (`filter_width < a->ne[2]`) hit 3 times in 9.3 hours of audio with VAD spans, and never in 15 hours without them, on one synthetic and one real file. It depends on the model's output, so it can't be predicted. Whisper needs a worker process with saved progress, as for pdfium. The harness redoes the crashed span on token times, which finished both files, at the cost of less accurate times for that span and 1.85 GB when the second context loads.
- **VAD must be called directly.** `whisper-rs`'s `state.full` calls `whisper_full_with_state`, which ignores `params.vad` with no warning. Use `WhisperVadContext`. Without VAD, whisper repeats phrases during lulls, and 60 s of silence gives "Thank you. Thank you."; with it, English 60-minute insertions fall from 940 to 130, and silence gives 0 words.
- **Word ends.** DTW gives starts only. A word's end is the next word's start, capped at 1.5 s.
- **Starts are late.** Local starts are a steady 150-210 ms late. Subtracting 170 ms gives 49-67 ms median, with 0-8% of words over 100 ms early (`tools/shift.py`). It isn't needed for the target, and spec 028 can use it or pad its playback ranges.
- **Decoding needs more than Symphonia's output.** Place samples by packet timestamps with holes as silence, 30 ms tolerance. Parse MP4 `elst` ourselves, because `isomp4` ignores it: without this, M4A is 23 ms off, MP4 46 ms, and late audio loses the whole 1.5 s. Trim Ogg pre-skip on the first packet. Resample with our own windowed-sinc filter.
- **Decoding can lose audio silently.** Symphonia rejects every MP3 packet after a mid-file change from mono to stereo, and reports each as a skipped packet, so 80% of one test file vanished. The harness now fails a job over 1% bad packets. A truncated live WebM, MP3, Opus or WAV still decodes. Other damaged files fail with an error and no crash: empty, garbage, truncated MP4 and finalised WebM, bit-flipped M4A and WebM, and video with no audio track.
- **Retiming.** Align words in runs of at least 2, interpolate between them, and refuse the result if under 50% of API words match. 85-97.5% matched with the chosen model. `base-q5` also meets the median, but 5.0% of Spanish words are off by over 1 s.
- **Licence gate.** The root `deny.toml` doesn't allow `Unlicense`, so `cargo deny` fails until spec 028 adds it.
- **`ort`.** `whisper-rs` and `ort` =2.0.0-rc.13 link into one binary and both initialise, with system libraries only.

Verification:
1. Done, above.
2. Passes for both chosen options: median 150-212 ms in every language, format, noise level and length. WER is recorded per language, and Swedish is the weakest at 10-15% locally. Gemini's own times fail, at 1,608 ms on Spanish.
3. Passes. All formats decode in process, in all 4 languages. The one gap is an MP3 changing channel layout mid-file, and the workaround is failing the job loudly.
4. Passes: libiconv, libc++, libSystem, libobjc, Accelerate, Foundation, CoreFoundation, Metal and MetalKit. Inference also ran under `sandbox-exec` with network denied.
5. Passes: at most 224 ms locally and 205 ms for the API on the local clock, against synthetic truth, by the end of 60 minutes. The real court recording shows 240 ms on pause points. Gemini's own times fail at 79.8 s.
6. Done.

**Corpus change:** timing truth is synthesizer word markers plus machine-derived pause points, not hand labels, and real multi-speaker audio is English only (`corpus.md`). The whole test sent synthetic and public-domain audio to Gemini, and cost $5.98.
