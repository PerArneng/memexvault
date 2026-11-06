#!/bin/bash
# Spec 027: Gemini over synthetic and public-domain audio only. Each run has its own token cap; total at most 2.2M.
cd "$(dirname "$0")/.."
export GEMINI_API_KEY=$(grep -E '^GEMINI_API_KEY=' "$1" | cut -d= -f2- | tr -d '"')
B=bench/target/release/asr-bench; M=gemini-3.8-flash; S=corpus/synth; R=corpus/real
run() { c=$1; l=$2; f=$3; cap=$4; shift 4; mkdir -p out/$c; n=$(basename "$f"); [ -f out/$c/$n.json ] || $B run --backend gemini --model $M --lang $l --file "$f" --out out/$c/$n.json --max-tokens $cap "$@" 2>&1 | cut -c1-260; }
(for l in sv de es; do run gemini-3.8-flash $l $S/${l}_5m.wav 70000; done; run gemini-3.8-flash-60s en $S/en_5m.wav 70000 --chunk 60) &
(for l in en sv de es; do run gemini-noise $l $S/${l}_5m_noise.wav 70000; done) &
(run gemini-real en $R/en_scotus_10m.mp3 130000; run gemini-real sv $R/sv_librivox_10m.mp3 130000; run gemini-real de $R/de_librivox_10m.mp3 130000; run gemini-real es $R/es_librivox_10m.mp3 130000) &
(run gemini-long en $S/en_60m.wav 650000) &
(run gemini-long sv $S/sv_60m.wav 650000) &
wait; echo DONE
