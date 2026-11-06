# Spec 027 corpus

Synthetic and public-domain audio only. No real meeting was sent to any provider. Not committed (`corpus/` is git-ignored). Rebuild the synthetic part with `tools/gen_synth.py`.

- **Synthetic, per language (`tools/gen_synth.py`):** English, Swedish, German and Spanish speech from the macOS speech synthesizer, four speakers taking turns.
  - **Files:** `<lang>_5m.wav`, `<lang>_5m_noise.wav` (250 ms reverb, pink noise and babble at 10 dB), `<lang>_60m.wav` (same at 20 dB, with lulls of 15-40 s every 4-12 minutes).
  - **Formats of each 5-minute file:** MP3, M4A, Opus, WebM and MP4 video.
  - **Edge files:** WebM, M4A and MP4 whose audio starts 1.5 s late, live WebM with no cues or duration, WebM with a 3 s hole, VBR MP3.
  - **Truth:** word times come from the synthesizer's markers, as sample offsets snapped to the voiced part of each word. Sentences whose markers don't cover the full text are rejected (46 in total).
  - **Text:** Project Gutenberg books: en 1661, sv 70770, de 5323, es 17340.
- **Real (`corpus/real/`):**
  - **English:** US Supreme Court oral argument 22-1008, 70.2 minutes, many speakers, a US government work, with the [audio](https://www.supremecourt.gov/media/audio/mp3files/22-1008.mp3) and [official transcript](https://www.supremecourt.gov/oral_arguments/argument_transcripts/2023/22-1008_f3b7.pdf).
  - **Swedish:** LibriVox, Baron Olson chapters 3 and 5, 14.5 minutes. Reference text is Gutenberg 15719.
  - **German:** LibriVox, Die Verwandlung chapter 1, 41 minutes. Reference text is Gutenberg 22367.
  - **Spanish:** LibriVox, Don Quijote volume 1 sections 1 and 2, 34.9 minutes, joined without re-encoding. Reference text is Gutenberg 2000. The first section is mono and the second joint stereo, and Symphonia rejects every packet after that change. So `es_librivox.mp3` and its 10-minute cut only decode the first 7 minutes, and that's all that was scored and sent to Gemini. Section 2 is also cut out on its own as `es_librivox_ch2.mp3`, 27.8 minutes, which was run locally.
  - **Sent to Gemini:** the first 10 minutes of each (`*_10m.mp3`), to limit cost, which is 7 minutes for Spanish as above.
- **Damaged files (`out/bad/`):** empty, garbage, truncated and bit-flipped versions of each format, a video with no audio track, and 60 s of silence.

**Changes from the spec:**
- **Timing truth isn't hand-labelled.** Synthetic files have truth for every word from the synthesizer, which is stronger than hand labels. Real files have machine-derived points instead: speech onsets after a pause of at least 400 ms in the audio. These points only catch small errors, because they're scored against the nearest word: Gemini's Spanish file scores a 105 ms median on them against 1,608 ms on word truth. So the synthetic figures decide. Real audio adds a stricter check, the share of word starts inside a pause, and a comparison of the two systems' clocks.
- **Real multi-speaker audio is English only.** The Swedish, German and Spanish real files have one reader each. Their only multi-speaker audio is synthetic.
- **Swedish has one synthetic voice.** Its four speakers are that voice at different speeds, so speaker separation in Swedish isn't a fair test.
- **Reference text is book or court text.** The synthetic words are books, not meeting vocabulary. Reference editions can differ from what the reader said, which adds to the real WER.
