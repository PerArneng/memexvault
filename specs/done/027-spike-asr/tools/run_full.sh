#!/bin/bash
# Spec 027: the chosen local configuration over the whole corpus, then the variants that decide timing and chunking.
cd "$(dirname "$0")/.."
B=bench/target/release/asr-bench; S=corpus/synth; R=corpus/real; T=models/ggml-large-v3-turbo-q5_0.bin
run() { c=$1; l=$2; f=$3; shift 3; mkdir -p out/$c; n=$(basename "$f"); [ -f out/$c/$n.json ] || $B run --backend whisper --model $T --lang $l --file "$f" --out out/$c/$n.json "$@" 2>>out/whisper.log; }
GPU="--gpu --dtw turbo"
for l in en sv de es; do
  for e in mp3 m4a opus webm mp4; do run formats $l $S/${l}_5m.$e $GPU; done
  run noise $l $S/${l}_5m_noise.wav $GPU
  run tokentime $l $S/${l}_5m.wav --gpu
  run vad $l $S/${l}_5m.wav $GPU --vad models/ggml-silero-v6.2.0.bin
done
for l in en sv de es; do run formats $l $S/${l}_edge_webm_late.webm $GPU; done
run real en $R/en_scotus.mp3 $GPU
run real sv $R/sv_librivox.mp3 $GPU
run real de $R/de_librivox.mp3 $GPU
run real es $R/es_librivox.mp3 $GPU
for l in en sv de es; do
  run long $l $S/${l}_60m.wav $GPU
  run long-chunk300 $l $S/${l}_60m.wav $GPU --chunk 300
  run long-vad $l $S/${l}_60m.wav $GPU --vad models/ggml-silero-v6.2.0.bin
done
for t in 4 8; do for l in en sv; do run cpu$t $l $S/${l}_5m.wav --dtw turbo --threads $t; done; done
echo DONE
