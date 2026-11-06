#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = ["numpy", "rapidfuzz", "soundfile"]
# ///
"""Spec 027: score transcripts against synthetic word truth or real reference text.

Usage: score.py out/<config>/<file>.json ... [--drift] [--points]
Synthetic files have truth per word. WER is word edits over truth words after lowercasing and removing punctuation.
Timing error is measured on words the alignment marks equal. Real files have reference text and pause-onset points:
a pause of at least 400 ms in the decoded audio, followed by speech, is a point, and the error is the distance from it to the
start of the nearest matched word within 1.5 s. That is machine-derived, and it's also run on synthetic files to check it agrees with the markers.
"""
import argparse
import bisect
import json
import re
import subprocess
import tempfile
import unicodedata
from collections import Counter, defaultdict
from pathlib import Path

import numpy as np
from rapidfuzz.distance import Levenshtein

ROOT = Path(__file__).resolve().parent.parent
BIN = ROOT / "bench/target/release/asr-bench"
HOMOPHONES = {"hvad": "vad", "hvarför": "varför", "af": "av"}


def norm(w: str) -> str:
    w = unicodedata.normalize("NFC", w).lower()
    w = re.sub(r"[^\w]", "", w).replace("_", "")
    return HOMOPHONES.get(w, w)


def align(ref: list[str], hyp: list[str], skip_ref_runs: int = 0) -> tuple[int, list[tuple[int, int]], dict]:
    """Edit distance, equal pairs (ref index, hyp index) and counts. With skip_ref_runs, a deletion run at least that long
    is book text the recording doesn't hold, such as a chapter it skips, and doesn't count; its size is in edits["skipped"]."""
    ops = Levenshtein.opcodes(ref, hyp)
    pairs, edits = [], {"sub": 0, "del": 0, "ins": 0}
    skipped = 0
    for o in ops:
        if skip_ref_runs and o.tag == "delete" and o.src_end - o.src_start >= skip_ref_runs:
            skipped += o.src_end - o.src_start
            continue
        if o.tag == "equal":
            pairs.extend(zip(range(o.src_start, o.src_end), range(o.dest_start, o.dest_end)))
        elif o.tag == "replace":
            n, m = o.src_end - o.src_start, o.dest_end - o.dest_start
            edits["sub"] += min(n, m)
            edits["del"] += max(n - m, 0)
            edits["ins"] += max(m - n, 0)
        elif o.tag == "delete":
            edits["del"] += o.src_end - o.src_start
        elif o.tag == "insert":
            edits["ins"] += o.dest_end - o.dest_start
    dist = sum(edits.values())
    if skip_ref_runs:
        edits["skipped"] = skipped
    return dist, pairs, edits


def pct(a: np.ndarray, q: float) -> float:
    return float(np.percentile(a, q)) if len(a) else float("nan")


def reference_text(name: str) -> list[str] | None:
    t = ROOT / "corpus/text"
    if name.startswith("en_scotus"):
        lines = []
        for l in (ROOT / "corpus/real/en_scotus.txt").read_text(encoding="utf-8").splitlines():
            m = re.match(r"\s*\d+\s+(.*)$", l)
            if m and "Heritage Reporting" not in l and "Official - Subject" not in l:
                lines.append(m.group(1))
        s = " ".join(lines)
        a = s.find("P R O C E E D I N G S")
        b = s.rfind("(Whereupon")
        s = s[a + 20 : b]
        s = re.sub(r"\b(CHIEF JUSTICE ROBERTS|JUSTICE [A-Z]+|M[RS]+\. [A-Z]+|GENERAL [A-Z]+)\s*:", " ", s)
        return [w for w in map(norm, s.split()) if w]
    src = {"sv": "sv_ref", "de": "de_ref", "es": "es_ref"}.get(name[:2])
    if not src:
        return None
    raw = (t / f"{src}.txt").read_text(encoding="utf-8")
    raw = re.split(r"\*\*\* ?START OF[^\n]*\n", raw, maxsplit=1)[-1]
    raw = re.split(r"\*\*\* ?END OF", raw, maxsplit=1)[0]
    return [w for w in map(norm, raw.split()) if w]


def window_real(ref: list[str], hyp: list[str]) -> tuple[list[str], int]:
    """Books hold far more text than the recording, so vote on where 6-word runs of the transcript occur in the book."""
    index = defaultdict(list)
    for i in range(len(ref) - 5):
        index[tuple(ref[i : i + 6])].append(i)
    hits = []
    for j in range(len(hyp) - 5):
        for i in index.get(tuple(hyp[j : j + 6]), [])[:3]:
            hits.append((i, j))
    if not hits:
        return ref[:1], 0
    starts = sorted(i - j for i, j in hits)
    lo = pct(np.array([i for i, _ in hits]), 1)
    hi = pct(np.array([i for i, _ in hits]), 99)
    a, b = max(int(lo) - 20, 0), min(int(hi) + 20, len(ref))
    return ref[a:b], a


def real_wer(ref: list[str], hyp: list[str]) -> dict:
    """WER over stretches between 6-word runs found in both, in order. A book holds far more than the recording, and a reader skips
    chapters and adds intros, so a stretch where the lengths differ by more than half and 30 words isn't compared; it's reported as skipped."""
    index = defaultdict(list)
    for i in range(len(ref) - 5):
        index[tuple(ref[i : i + 6])].append(i)
    hits = [(i, j) for j in range(len(hyp) - 5) for i in index.get(tuple(hyp[j : j + 6]), [])[:5]]
    # The longest chain increasing in both, which drops chance repeats elsewhere in the book.
    hits.sort(key=lambda h: (h[1], h[0]))
    tails, prev, ids = [], [-1] * len(hits), []
    for k, (i, _) in enumerate(hits):
        p = bisect.bisect_left(tails, i)
        prev[k] = ids[p - 1] if p else -1
        if p == len(tails):
            tails.append(i), ids.append(k)
        else:
            tails[p], ids[p] = i, k
    chain, k = [], ids[-1] if ids else -1
    while k >= 0:
        chain.append(hits[k])
        k = prev[k]
    chain.reverse()
    if not chain:
        return {}
    edits, match, words, skipped_r, skipped_h = Counter(), 0, 0, 0, 0
    for (i0, j0), (i1, j1) in zip(chain, chain[1:]):
        a, b = i1 - i0, j1 - j0
        if abs(a - b) > max(a, b) / 2 and abs(a - b) > 30:
            skipped_r += a
            skipped_h += b
            continue
        if a <= 0 or b <= 0:
            continue
        _, pairs, e = align(ref[i0:i1], hyp[j0:j1])
        edits.update(e)
        match += len(pairs)
        words += a
    return {"wer": sum(edits.values()) / max(words, 1), "sub": edits["sub"], "del": edits["del"], "ins": edits["ins"], "ref_words": words, "matched": match / max(words, 1), "skipped_ref": skipped_r, "skipped_hyp": skipped_h}


def pause_starts(path: dict, pcm: np.ndarray) -> float:
    """Share of word starts more than 200 ms inside a pause of at least 400 ms. A correct clock has few, whichever system is right."""
    hop = 160
    n = len(pcm) // hop
    db = 20 * np.log10(np.sqrt(np.mean(pcm[: n * hop].reshape(n, hop) ** 2, axis=1) + 1e-12))
    th = np.percentile(db, 5) + (np.percentile(db, 95) - np.percentile(db, 5)) * 0.3
    voiced = db > th
    inside = np.zeros(n, dtype=bool)
    i = 0
    while i < n:
        if voiced[i]:
            i += 1
            continue
        j = i
        while j < n and not voiced[j]:
            j += 1
        if j - i >= 40:
            inside[i + 20 : max(j - 20, i + 20)] = True
        i = j
    s = np.array([w["s"] // 10 for w in path["words"] if w.get("s") is not None and 0 <= w["s"] // 10 < n], dtype=int)
    return float(inside[s].mean()) if len(s) else float("nan")


def pause_points(pcm: np.ndarray, min_pause_ms: int = 400) -> np.ndarray:
    """Onsets in ms of speech after a pause, from 10 ms RMS against a floor taken from the file."""
    hop = 160
    n = len(pcm) // hop
    e = np.sqrt(np.mean(pcm[: n * hop].reshape(n, hop) ** 2, axis=1) + 1e-12)
    db = 20 * np.log10(e)
    floor, peak = np.percentile(db, 5), np.percentile(db, 95)
    th = floor + (peak - floor) * 0.3
    voiced = db > th
    points = []
    run = 0
    for i, v in enumerate(voiced):
        if v:
            # needs 100 ms of sustained speech, so a click isn't a point
            if run * 10 >= min_pause_ms and voiced[i : i + 10].mean() > 0.8:
                points.append(i * 10)
            run = 0
        else:
            run += 1
    return np.array(points, dtype=np.int64)


def decoded(file: str) -> np.ndarray:
    with tempfile.NamedTemporaryFile(suffix=".f32") as t:
        subprocess.run([str(BIN), "decode", "--file", file, "--out", t.name], capture_output=True, check=True)
        return np.fromfile(t.name, dtype=np.float32)


def score_file(path: Path, do_points: bool) -> dict:
    o = json.loads(path.read_text())
    file = o["info"]["file"]
    stem = Path(file).name.split(".")[0]
    # A file's truth is its master's: en_5m.opus and en_edge_x share en_5m's words, some with a known shift.
    truth_path = ROOT / "corpus/synth" / f"{stem.replace('_edge_webm_late', '_5m')}.json"
    shift = 1500 if "_edge_webm_late" in stem else 0  # that file starts its audio 1.5 s late
    hyp = [(norm(w["w"]), w) for w in o["words"]]
    hyp = [(n, w) for n, w in hyp if n]
    hw = [n for n, _ in hyp]
    r: dict = {"name": f"{path.parent.name}/{path.stem}", "x_rt": o.get("x_realtime"), "rss": o.get("peak_rss_mb"), "tok_in": o.get("input_tokens"), "tok_out": o.get("output_tokens"), "failed": o.get("failed_chunks"), "hyp_words": len(hw)}
    t = json.loads(truth_path.read_text()) if truth_path.exists() else None
    if t:
        tw = [(norm(w["w"]), w) for w in t["words"]]
        tw = [(n, w) for n, w in tw if n]
        rw = [n for n, _ in tw]
        dist, pairs, edits = align(rw, hw)
        r |= {"wer": dist / len(rw), **edits, "ref_words": len(rw), "matched": len(pairs) / len(rw)}
        ds = np.array([hyp[j][1]["s"] - shift - tw[i][1]["s"] for i, j in pairs], dtype=float)
        de = np.array([hyp[j][1]["e"] - shift - tw[i][1]["e"] for i, j in pairs], dtype=float)
        ts = np.array([tw[i][1]["s"] for i, _ in pairs], dtype=float)
        r |= {"start_med": pct(np.abs(ds), 50), "start_p90": pct(np.abs(ds), 90), "start_p99": pct(np.abs(ds), 99), "start_signed": pct(ds, 50), "end_med": pct(np.abs(de), 50), "end_p90": pct(np.abs(de), 90), "over_1s": float((np.abs(ds) > 1000).mean()) if len(ds) else None}
        bucket = 300_000 if ts.max(initial=0) > 900_000 else 60_000
        drift = []
        for b in range(0, int(ts.max(initial=0)) + 1, bucket):
            m = (ts >= b) & (ts < b + bucket)
            if m.sum() >= 10:
                drift.append((b // 60000, round(pct(ds[m], 50))))
        r["drift"] = drift
        r["drift_max"] = max((abs(d) for _, d in drift), default=None)
        spk = [(tw[i][1]["spk"], hyp[j][1].get("spk")) for i, j in pairs if hyp[j][1].get("spk")]
        if spk:
            by = defaultdict(Counter)
            for a, b in spk:
                by[b][a] += 1
            r["spk_purity"] = sum(c.most_common(1)[0][1] for c in by.values()) / len(spk)
            r["spk_labels"] = len(by)
            r["spk_true"] = len({a for a, _ in spk})
    else:
        ref = reference_text(stem)
        if ref:
            r |= real_wer(ref, hw)
    if do_points:
        pcm = decoded(file)
        pts = pause_points(pcm)
        r["in_pause"] = pause_starts(o, pcm)
        starts = np.array(sorted(w["s"] for _, w in hyp), dtype=float)
        if len(starts) and len(pts):
            idx = np.clip(np.searchsorted(starts, pts), 1, len(starts) - 1)
            near = np.where(np.abs(starts[idx] - pts) < np.abs(starts[idx - 1] - pts), starts[idx], starts[idx - 1])
            d = near - pts
            keep = np.abs(d) <= 1500
            r |= {"points": int(keep.sum()), "points_all": len(pts), "point_med": pct(np.abs(d[keep]), 50), "point_p90": pct(np.abs(d[keep]), 90), "point_signed": pct(d[keep], 50)}
            bucket = 300_000
            drift = []
            for b in range(0, int(pts.max()) + 1, bucket):
                m = keep & (pts >= b) & (pts < b + bucket)
                if m.sum() >= 5:
                    drift.append((b // 60000, round(pct(d[m], 50))))
            r["point_drift"] = drift
            r["point_drift_max"] = max((abs(x) for _, x in drift), default=None)
    return r


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("paths", nargs="+")
    ap.add_argument("--points", action="store_true")
    ap.add_argument("--drift", action="store_true")
    a = ap.parse_args()
    print("file | x rt | rss MB | WER % (sub/del/ins) | matched % | start med/p90/p99 ms | signed | end med/p90 ms | >1 s % | drift max | speakers | points n med/p90 drift")
    for p in a.paths:
        r = score_file(Path(p), a.points)
        f = lambda k, s="{:.0f}": s.format(r[k]) if r.get(k) is not None else "-"
        wer = f"{r['wer'] * 100:.1f} ({r['sub']}/{r['del']}/{r['ins']})" if "wer" in r else "-"
        match = f"{r['matched'] * 100:.1f}" if "matched" in r else "-"
        spk = f"{r['spk_purity'] * 100:.0f}% {r['spk_labels']}/{r['spk_true']}" if "spk_purity" in r else "-"
        pts = f"{r['points']}/{r['points_all']} {r['point_med']:.0f}/{r['point_p90']:.0f} {r.get('point_drift_max')} in-pause {r.get('in_pause', 0) * 100:.1f}%" if "points" in r else "-"
        over = f"{r['over_1s'] * 100:.1f}" if r.get("over_1s") is not None else "-"
        skip = f" skipped {r['skipped_ref']}/{r['skipped_hyp']}" if "skipped_ref" in r else ""
        print(f"{r['name']}{skip} | {f('x_rt', '{:.1f}')} | {f('rss')} | {wer} | {match} | {f('start_med')}/{f('start_p90')}/{f('start_p99')} | {f('start_signed')} | {f('end_med')}/{f('end_p90')} | {over} | {f('drift_max')} | {spk} | {pts}", flush=True)
        if a.drift:
            print("  drift", r.get("drift"), "points", r.get("point_drift"), flush=True)


if __name__ == "__main__":
    main()
