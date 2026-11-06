#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = ["numpy", "scipy", "soundfile"]
# ///
"""Build the synthetic part of the spec 027 corpus.

Speech comes from macOS AVSpeechSynthesizer (out/tts, built from tools/tts.swift), whose markers give the sample offset of
every word, so timing truth is per word rather than a few labelled points. Sentences come from public-domain books.
Speakers take turns, with pauses and occasional long silences as in a meeting. ffmpeg only prepares the corpus here.

Usage: gen_synth.py [--only 5m,60m,formats]
Writes corpus/synth/<lang>_<kind>.<ext> and corpus/synth/<lang>_<kind>.json with {dur_ms, text, words:[{w,s,e,spk}]}.
"""
import argparse
import hashlib
import json
import random
import re
import subprocess
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import numpy as np
import soundfile as sf
from scipy.signal import fftconvolve, resample_poly

ROOT = Path(__file__).resolve().parent.parent
CORPUS = ROOT / "corpus"
OUT = CORPUS / "synth"
CACHE = CORPUS / "tts_cache"
SR = 22050
REJECTED = [0]
A = "com.apple."
# (voice id, speed factor applied afterwards). Swedish has one voice, so its extra speakers are resampled copies.
VOICES = {
    "en": [(A + "voice.compact.en-US.Samantha", 1.0), (A + "voice.compact.en-GB.Daniel", 1.0), (A + "eloquence.en-US.Eddy", 1.0), (A + "eloquence.en-GB.Flo", 1.0)],
    "sv": [(A + "voice.compact.sv-SE.Alva", 1.0), (A + "voice.compact.sv-SE.Alva", 0.94), (A + "voice.compact.sv-SE.Alva", 0.88), (A + "voice.compact.sv-SE.Alva", 1.1)],
    "de": [(A + "voice.compact.de-DE.Anna", 1.0), (A + "eloquence.de-DE.Eddy", 1.0), (A + "eloquence.de-DE.Flo", 1.0), (A + "eloquence.de-DE.Rocko", 1.0)],
    "es": [(A + "voice.compact.es-ES.Monica", 1.0), (A + "voice.compact.es-MX.Paulina", 1.0), (A + "eloquence.es-ES.Eddy", 1.0), (A + "eloquence.es-MX.Flo", 1.0)],
}


def sentences(lang: str) -> list[str]:
    raw = (CORPUS / "text" / f"{lang}.txt").read_text(encoding="utf-8")
    raw = re.split(r"\*\*\* ?START OF[^\n]*\n", raw, maxsplit=1)[-1]
    raw = re.split(r"\*\*\* ?END OF", raw, maxsplit=1)[0]
    out = []
    for para in re.split(r"\n\s*\n", raw):
        para = re.sub(r"[_*\[\]“”„«»\"]|--+|—|–", " ", para)
        para = re.sub(r"\s+", " ", para).strip()
        for s in re.split(r"(?<=[.!?])\s+(?=[A-ZÅÄÖÜÉÑ¡¿])", para):
            s = s.strip(" -'’")
            n = len(s.split())
            if 3 <= n <= 32 and not re.search(r"\d", s) and s[-1] in ".!?" and s.upper() != s:
                out.append(s)
    return out


def synth_all(jobs: list[tuple[str, str]]) -> None:
    """jobs: (voice, text). Writes cache files that are missing, in 8 shards."""
    wanted = {key_of(voice, text): (voice, text) for voice, text in jobs}

    def shard(part: list) -> None:
        lines = "".join(json.dumps({"id": k, "voice": v, "text": t, "out": str(CACHE / f"{k}.caf")}, ensure_ascii=False) + "\n" for k, (v, t) in part)
        r = subprocess.run([str(ROOT / "out" / "tts")], input=lines, capture_output=True, text=True)
        if r.returncode != 0:
            print("tts died", r.returncode, r.stderr[-160:].strip(), flush=True)
        for line in r.stdout.splitlines():
            o = json.loads(line)
            (CACHE / f"{o['id']}.json").write_text(json.dumps(o, ensure_ascii=False))

    # The system speech service sometimes crashes the helper, so rerun what is left. What never succeeds is skipped.
    for attempt in range(12):
        items = [(k, v) for k, v in wanted.items() if not (CACHE / f"{k}.json").exists()]
        if not items:
            return
        print(f"synth {len(items)} sentences, attempt {attempt}", flush=True)
        random.Random(attempt).shuffle(items)
        with ThreadPoolExecutor(8) as ex:
            list(ex.map(shard, [items[i::8] for i in range(8)]))
    print("skipped sentences", sum(not (CACHE / f"{k}.json").exists() for k in wanted), flush=True)


def key_of(voice: str, text: str) -> str:
    return hashlib.sha1(f"{voice}\n{text}".encode()).hexdigest()[:20]


def load(voice: str, factor: float, text: str) -> tuple[np.ndarray, list[dict]] | None:
    """Mono audio at SR and words with sample positions, snapped to the voiced part next to each marker."""
    key = key_of(voice, text)
    if not (CACHE / f"{key}.json").exists():
        return None
    m = json.loads((CACHE / f"{key}.json").read_text())
    if m["frames"] == 0 or not m["words"]:
        return None
    letters = lambda t: re.sub(r"[^\w]", "", t).lower()
    if letters("".join(w["t"] for w in m["words"])) != letters(text):
        REJECTED[0] += 1  # markers don't cover every word, so this sentence's truth would be wrong
        return None
    x, rate = sf.read(CACHE / f"{key}.caf", dtype="float32")
    if x.ndim > 1:
        x = x.mean(axis=1)
    assert len(x) == m["frames"], f"{key}: {len(x)} frames, markers say {m['frames']}"
    scale = SR / rate / factor
    source_rate = int(round(rate * factor))  # playing at another speed is how the extra Swedish speakers are made
    x = resample_poly(x, SR, source_rate) if source_rate != SR else x
    frame = int(SR * 0.005)
    n = len(x) // frame
    if n == 0:
        return None
    env = np.sqrt(np.mean(x[: n * frame].reshape(n, frame) ** 2, axis=1))
    voiced = env > max(env.max() * 0.03, 1e-4)
    marks = [int(w["byte"] / 4 * scale) for w in m["words"]] + [len(x)]
    words = []
    for i, w in enumerate(m["words"]):
        a, b = marks[i] // frame, max(marks[i + 1] // frame, marks[i] // frame + 1)
        idx = np.nonzero(voiced[a:b])[0]
        if len(idx) == 0:
            s, e = marks[i], marks[i + 1]
        else:
            s, e = (a + idx[0]) * frame, (a + idx[-1] + 1) * frame
        words.append({"w": w["t"], "s": s, "e": e, "mark": marks[i]})
    return x, words


def build_timeline(lang: str, sents: list[str], minutes: float, seed: int) -> tuple[np.ndarray, list[dict], list[str]]:
    rng = random.Random(seed)
    voices = VOICES[lang]
    plan = []
    i = 0
    # about 3 words a second, with a margin so the timeline can be filled
    need = int(minutes * 60 * 3.4)
    words = 0
    spk = 0
    while words < need and i < len(sents):
        spk = rng.choice([v for v in range(len(voices)) if v != spk])
        for _ in range(rng.randint(1, 4)):
            if i >= len(sents):
                break
            plan.append((spk, sents[i]))
            words += len(sents[i].split())
            i += 1
    synth_all([(voices[s][0], t) for s, t in plan])
    total = int(minutes * 60 * SR)
    buf = np.zeros(total + SR * 60, dtype=np.float32)
    pos = SR
    truth: list[dict] = []
    used = []
    last = None
    next_lull = rng.uniform(4, 12) * 60 * SR
    for spk, text in plan:
        got = load(voices[spk][0], voices[spk][1], text)
        if got is None:
            continue
        x, ws = got
        pos += int(rng.uniform(0.1, 0.6) * SR) if spk == last else int(rng.uniform(0.3, 1.5) * SR)
        if pos > next_lull:  # a lull in the meeting, the case that makes models invent text
            pos += int(rng.uniform(15, 40) * SR)
            next_lull = pos + rng.uniform(4, 12) * 60 * SR
        if pos + len(x) >= total:
            break
        buf[pos : pos + len(x)] += x * rng.uniform(0.5, 0.95)
        for w in ws:
            truth.append({"w": w["w"], "s": round((pos + w["s"]) * 1000 / SR), "e": round((pos + w["e"]) * 1000 / SR), "spk": spk})
        used.append(text)
        pos += len(x)
        last = spk
    return buf[:total], truth, used


def pink(n: int, rng: np.random.Generator) -> np.ndarray:
    spec = np.fft.rfft(rng.standard_normal(n))
    f = np.arange(len(spec))
    f[0] = 1
    y = np.fft.irfft(spec / np.sqrt(f), n)
    return (y / np.std(y)).astype(np.float32)


def add_noise(x: np.ndarray, babble: np.ndarray, snr_db: float, seed: int) -> np.ndarray:
    """Room reverb, then pink noise and quiet other speakers at snr_db. Reverb tail is 250 ms."""
    rng = np.random.default_rng(seed)
    ir_n = int(0.25 * SR)
    ir = rng.standard_normal(ir_n) * np.exp(-np.arange(ir_n) / (0.05 * SR)) * 0.25
    ir[0] = 1.0
    wet = fftconvolve(x, ir)[: len(x)].astype(np.float32)
    active = np.abs(x) > 1e-4
    level = np.sqrt(np.mean(x[active] ** 2))
    gain = level / 10 ** (snr_db / 20)
    b = babble[: len(x)]
    b = b / max(np.std(b), 1e-6)
    y = wet + gain * (0.7 * pink(len(x), rng) + 0.7 * b)
    return y / max(np.abs(y).max() / 0.95, 1.0)


def write(name: str, x: np.ndarray, truth: list[dict], text: list[str]) -> None:
    OUT.mkdir(exist_ok=True, parents=True)
    sf.write(OUT / f"{name}.wav", x, SR, subtype="PCM_16")
    (OUT / f"{name}.json").write_text(json.dumps({"dur_ms": round(len(x) * 1000 / SR), "text": " ".join(text), "words": truth}, ensure_ascii=False))
    print(name, f"{len(x) / SR / 60:.1f} min", len(truth), "words, sentences rejected so far", REJECTED[0], flush=True)


def ff(*args: str) -> None:
    subprocess.run(["ffmpeg", "-y", "-v", "error", *args], check=True)


def formats(lang: str) -> None:
    s = str(OUT / f"{lang}_5m")
    w = s + ".wav"
    ff("-i", w, "-c:a", "libmp3lame", "-b:a", "64k", s + ".mp3")
    ff("-i", w, "-ac", "2", "-ar", "44100", "-c:a", "aac", "-b:a", "64k", s + ".m4a")
    ff("-i", w, "-c:a", "libopus", "-b:a", "24k", s + ".opus")
    ff("-i", w, "-ac", "2", "-c:a", "libopus", "-b:a", "32k", s + ".webm")
    ff("-f", "lavfi", "-i", "color=c=gray:s=320x240:r=5", "-i", w, "-shortest", "-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p", "-c:a", "aac", "-b:a", "64k", s + ".mp4")


def edge_cases(lang: str) -> None:
    """Timeline traps real recorders produce. Expected shift against the master, in ms, is in the file name's table in score.py."""
    s = str(OUT / f"{lang}_5m")
    e = str(OUT / f"{lang}_edge")
    ff("-itsoffset", "1.5", "-i", s + ".webm", "-c", "copy", e + "_webm_late.webm")
    ff("-itsoffset", "1.5", "-i", s + ".m4a", "-c", "copy", e + "_m4a_late.m4a")
    ff("-f", "lavfi", "-i", "color=c=gray:s=320x240:r=5:d=300", "-itsoffset", "1.5", "-i", s + ".m4a", "-map", "0:v", "-map", "1:a", "-c:v", "libx264", "-preset", "ultrafast", "-pix_fmt", "yuv420p", "-c:a", "copy", e + "_mp4_late.mp4")
    # what browsers' MediaRecorder and similar write: no duration, no cues
    ff("-i", s + ".webm", "-c", "copy", "-f", "webm", "-live", "1", "-cluster_time_limit", "1000", e + "_webm_live.webm")
    # a 3 s hole in the packets after 60 s, as when a WebRTC recorder loses the network
    ff("-i", s + ".webm", "-c", "copy", "-bsf:a", "setts=ts='if(gt(TS\\,60000)\\,TS+3000\\,TS)'", e + "_webm_gap.webm")
    ff("-i", s + ".wav", "-ac", "2", "-c:a", "libmp3lame", "-q:a", "6", e + "_mp3_vbr.mp3")


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--only", default="5m,formats,60m")
    a = ap.parse_args()
    only = set(a.only.split(","))
    CACHE.mkdir(parents=True, exist_ok=True)
    for li, lang in enumerate(VOICES):
        sents = sentences(lang)
        random.Random(100 + li).shuffle(sents)
        print(lang, len(sents), "sentences", sum(len(s.split()) for s in sents), "words", flush=True)
        n5 = 200
        if "5m" in only:
            x, t, used = build_timeline(lang, sents[:n5], 5, 1 + li)
            write(f"{lang}_5m", x, t, used)
            b, _, _ = build_timeline(lang, sents[n5 : 2 * n5], 5, 11 + li)
            write(f"{lang}_5m_noise", add_noise(x, b, 10.0, 21 + li), t, used)
        if "formats" in only:
            formats(lang)
            edge_cases(lang)
        if "60m" in only:
            x, t, used = build_timeline(lang, sents[2 * n5 :], 60, 31 + li)
            b, _, _ = build_timeline(lang, sents[:n5], 5, 41 + li)
            write(f"{lang}_60m", add_noise(x, np.tile(b, 13)[: len(x)], 20.0, 51 + li), t, used)


if __name__ == "__main__":
    main()
