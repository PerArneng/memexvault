#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = ["numpy", "scipy"]
# ///
"""Spec 027: decode every format in-process and measure its timeline against the WAV master.

Usage: score_decode.py > ../decode_results.txt
Lag is the cross-correlation peak of the waveform at 30 s, the middle and the last minute: positive means later than the master.
"""
import json
import subprocess
import tempfile
from pathlib import Path

import numpy as np
from scipy.signal import correlate

ROOT = Path(__file__).resolve().parent.parent
BIN = ROOT / "bench/target/release/asr-bench"
S = ROOT / "corpus/synth"
R = 16000
# name: (expected ms shift, after which second the shift applies, or None for everywhere)
FILES = {
    "5m.wav": (0, 0), "5m.mp3": (0, 0), "5m.m4a": (0, 0), "5m.opus": (0, 0), "5m.webm": (0, 0), "5m.mp4": (0, 0),
    "edge_mp3_vbr.mp3": (0, 0), "edge_webm_live.webm": (0, 0), "edge_webm_late.webm": (1500, 0), "edge_m4a_late.m4a": (1500, 0),
    "edge_mp4_late.mp4": (1500, 0), "edge_webm_gap.webm": (3000, 60),
}


def decode(path: Path, *flags: str) -> tuple[dict, np.ndarray]:
    with tempfile.NamedTemporaryFile(suffix=".f32") as t:
        r = subprocess.run([str(BIN), "decode", "--file", str(path), "--out", t.name, *flags], capture_output=True, text=True, check=True)
        o = json.loads(r.stdout)
        return o, np.fromfile(t.name, dtype=np.float32) if "info" in o else np.zeros(0)


def lag_ms(master: np.ndarray, x: np.ndarray, at_s: float, expect_ms: float) -> float:
    n = R * 6
    a = int(at_s * R)
    ref = master[a : a + n]
    search = R * 5
    b = max(a + int(expect_ms / 1000 * R) - search, 0)
    seg = x[b : b + n + 2 * search]
    if len(seg) < n or np.std(ref) == 0:
        return float("nan")
    c = correlate(seg - seg.mean(), ref - ref.mean(), mode="valid", method="fft")
    return (b + int(np.argmax(c)) - a) * 1000 / R


def main() -> None:
    print("file | codec | ch | rate | dur s | gaps | bad | lag at 30 s / mid / end ms | expected | decode x realtime | rss MB")
    worst = 0.0
    for lang in ["en", "sv", "de", "es"]:
        _, master = decode(S / f"{lang}_5m.wav")
        for name, (exp, after) in FILES.items():
            for flags in ([], ["--no-elst"]) if name.endswith(("m4a", "mp4")) else ([],):
                o, x = decode(S / f"{lang}_{name}", *flags)
                if "error" in o:
                    print(lang, name, "ERROR", o["error"])
                    continue
                i = o["info"]
                lags = [lag_ms(master, x, t, exp if t >= after else 0) for t in (30, 150, 280)]
                err = max(abs(l - (exp if t >= after else 0)) for l, t in zip(lags, (30, 150, 280)))
                if not flags:
                    worst = max(worst, err)
                print(f"{lang}_{name}{' (edit list ignored)' if flags else ''} | {i['codec']} | {i['channels']} | {i['rate']} | {i['dur_s']:.2f} | {i['gaps']}/{i['gap_ms']:.0f} ms | {i['bad_packets']} | "
                      + " / ".join(f"{l:.1f}" for l in lags) + f" | {exp} | {i['dur_s'] / i['decode_s']:.0f} | {o['peak_rss_mb']:.0f}", flush=True)
    print(f"worst timeline error, edit lists read: {worst:.1f} ms")


if __name__ == "__main__":
    main()
