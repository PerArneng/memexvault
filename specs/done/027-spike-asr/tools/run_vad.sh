#!/bin/bash
# Spec 027: the chosen local configuration with Silero VAD run by the harness (run_full.sh's --vad runs were ignored by whisper_full_with_state).
cd "$(dirname "$0")/.."
B=bench/target/release/asr-bench; S=corpus/synth; R=corpus/real; T=models/ggml-large-v3-turbo-q5_0.bin
run() { c=$1; l=$2; f=$3; mkdir -p out/$c; n=$(basename "$f"); [ -f out/$c/$n.json ] || $B run --backend whisper --model $T --lang $l --file "$f" --out out/$c/$n.json --gpu --dtw turbo --vad models/ggml-silero-v6.2.0.bin 2>>out/whisper.log; }
run vad-bad en out/bad/silence.wav
for l in en sv de es; do
  run vad $l $S/${l}_5m.wav
  for e in mp3 m4a opus webm mp4; do run vad-formats $l $S/${l}_5m.$e; done
  run vad-formats $l $S/${l}_edge_webm_late.webm
  run vad-noise $l $S/${l}_5m_noise.wav
done
run vad-real en $R/en_scotus.mp3; run vad-real sv $R/sv_librivox.mp3; run vad-real de $R/de_librivox.mp3; run vad-real es $R/es_librivox.mp3
for l in en sv es; do run vad-long $l $S/${l}_60m.wav; done
# These two hit whisper.cpp's DTW assert, so they run again until done. --progress resumes and redoes the crashed span on token times.
resume() { c=$1; l=$2; f=$3; mkdir -p out/$c; n=$(basename "$f"); until $B run --backend whisper --model $T --lang $l --file "$f" --out out/$c/$n.json --gpu --dtw turbo --vad models/ggml-silero-v6.2.0.bin --progress out/$c/$n.progress 2>>out/whisper.log; do :; done; }
resume vad-long de $S/de_60m.wav
resume vad-real es $R/es_librivox_ch2.mp3
echo DONE
