#!/bin/bash
# Spec 027: whisper model comparison on the 5-minute masters. Whisper's stderr goes to out/whisper.log.
cd "$(dirname "$0")/.."
B=bench/target/release/asr-bench; S=corpus/synth
run() { # config model preset lang file extra...
  c=$1; m=$2; d=$3; l=$4; f=$5; shift 5; mkdir -p out/$c
  n=$(basename "$f"); [ -f out/$c/$n.json ] || $B run --backend whisper --model models/$m --gpu --dtw $d --lang $l --file "$f" --out out/$c/$n.json "$@" 2>>out/whisper.log
}
for l in en sv de es; do
  run base-q5 ggml-base-q5_1.bin base $l $S/${l}_5m.wav
  run small-q5 ggml-small-q5_1.bin small $l $S/${l}_5m.wav
  run turbo-q5 ggml-large-v3-turbo-q5_0.bin turbo $l $S/${l}_5m.wav
  run turbo ggml-large-v3-turbo.bin turbo $l $S/${l}_5m.wav
  run turbo-q5-beam5 ggml-large-v3-turbo-q5_0.bin turbo $l $S/${l}_5m.wav --beam 5
done
