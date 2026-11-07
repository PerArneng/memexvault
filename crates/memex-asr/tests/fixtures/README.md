# Decoder fixtures

Synthetic tones with no speech, made with `ffmpeg` from its own signal generators, so they hold no third-party content. They're only for testing decoding, as the decoder doesn't use `ffmpeg`.

- **`tone.*`:** 2 s, stereo at 48 kHz, silent for 1 s and then 440 Hz: `-f lavfi -i "aevalsrc=exprs=if(gte(t\,1)\,0.5*sin(2*PI*440*t)\,0):d=2:s=48000:c=stereo"`. `tone.mp4` also has a small video track, which comes first.
- **`late.m4a`:** `tone.m4a` with `-itsoffset 1.5`, so its edit list starts the audio 1.5 s late.
- **`no_audio.mp4`:** video only.
- **`mono_then_stereo.mp3`:** 8 s mono and 8 s stereo MP3 joined by `cat`. Symphonia rejects every packet after the change (spec 027).
