---
id: 061
name: review-asr-api-and-accuracy
description: "Decide whether the API transcriber's need for a local model and whisper's Swedish accuracy are acceptable"
---

# 061 review-asr-api-and-accuracy

## Goal
Have the user decide whether spec 027's transcription choice is good enough, in the four areas below where its measurements fall short of what a reader might assume.

## Background
**Depends on:** 027. Spec 027's Result has the numbers.

- **The API option needs the local model.** Gemini's own word times are off by a median of 1.6 s on Spanish, and up to 80 s in 60-minute files. On real audio, 19-52% of its word starts are over 1 s from the local model's. So spec 027 places Gemini's words on the local clock. The API improves text and adds speakers, at $1.41-1.71 per hour, or $2.82-3.42 from 2027-01-01. It doesn't remove the local model, its memory use or its time.
- **Only Gemini was tested as an API.** It was the only key available, and adding another provider needs the user's account and money.
- **Swedish accuracy.** Local WER is 10.0% on 5 minutes, 15.1% on 60 minutes and 11.0% on real audio. With Gemini's text it's 4.3%, 7.6% and 8.0%.
- **Evidence gaps.** Real multi-speaker audio is English only. Swedish has a single synthetic voice. No real meeting was tested, and timing truth isn't hand-labelled. On the real multi-speaker file, the local model dropped 735 of about 13,000 words, for a reason not yet known.

- **Spec 028 missed its citation target.** The cited range should contain the spoken quote, with the start within 1 s, for 95% of claims. It measured 90.0% on `en_5m` with 300 ms of padding, and then 92.8% of 708 claims on `en_60m` with 600 ms at the start, which decided it. Beside that, `sv_5m` 92.6%, `de_5m` 98.3% and `es_5m` 100.0%. No start was late on `en_60m`: the misses were 34 starts over 1 s early, 4 ends early and 15 claims under half aligned. The speech is synthetic, the labels are the synthesizer's markers and the agent is scripted, so hand-labelled real speech is what would settle it. Spec 028 landed with this unmet, by the user's decision.
- **The API transcriber hasn't run live in spec 028.** Only a fake answered it. The command and token estimate are in spec 028's Result, and running it sends audio to the provider.

**Assumption in force:** spec 027's choice stands. Spec 028 builds the local transcriber and the API on the local clock, with API use off unless configured.

## Approach
Ask the user whether an API that still needs the local model is acceptable, or if a provider with its own word timestamps should be tested. Ask whether Swedish at 10-15% is acceptable locally. Ask whether the user can test on a real meeting of theirs, locally only. Ask whether spec 028's 92.8% is acceptable, or what recording and labels should judge it next, without tuning padding further on the same synthetic audio.

## Verification
1. The user's answers are recorded here.
2. If a new provider is tested, the same corpus and `tools/score.py` from spec 027 are used, with synthetic and public-domain audio only.

## Out of scope
Building spec 028, and speaker identification by name.

## Result
Not started.
