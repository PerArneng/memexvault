#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = ["numpy", "rapidfuzz", "soundfile"]
# ///
"""Spec 027: put the API's words on a local model's clock, offline, from runs already made.

Usage: hybrid.py <api.json> <local.json> <out.json>
The two word lists are aligned. A matched API word takes the local word's times, and words between matches are interpolated
over their characters. Text and speaker labels stay the API's. The output is scored by score.py like any other run.
"""
import json
import sys
from importlib.machinery import SourceFileLoader
from pathlib import Path

from rapidfuzz.distance import Levenshtein

score = SourceFileLoader("score", str(Path(__file__).with_name("score.py"))).load_module()


def retime(api: list[dict], local: list[dict]) -> tuple[list[dict], float]:
    a = [(score.norm(w["w"]), w) for w in api]
    a = [x for x in a if x[0]]
    l = [(score.norm(w["w"]), w) for w in local]
    l = [x for x in l if x[0]]
    pairs = []
    for o in Levenshtein.opcodes([x for x, _ in a], [x for x, _ in l]):
        # A lone matching word between edits is often a chance match such as "the", so only runs of two or more count.
        if o.tag == "equal" and o.src_end - o.src_start >= 2:
            pairs.extend(zip(range(o.src_start, o.src_end), range(o.dest_start, o.dest_end)))
    fixed = {i: j for i, j in pairs}
    out = [dict(w) for _, w in a]
    idx = sorted(fixed)
    for i in idx:
        out[i]["s"], out[i]["e"] = l[fixed[i]][1]["s"], l[fixed[i]][1]["e"]
    # Words between two matches share the time between them. Before the first or after the last, they keep the API's offset to it.
    bounds = [-1, *idx, len(a)]
    for lo, hi in zip(bounds, bounds[1:]):
        if hi - lo <= 1:
            continue
        gap = range(lo + 1, hi)
        if lo < 0 or hi >= len(a):
            ref = hi if lo < 0 else lo
            if 0 <= ref < len(a) and ref in fixed:
                d = out[ref]["s"] - a[ref][1]["s"]
                for i in gap:
                    out[i]["s"] += d
                    out[i]["e"] += d
            continue
        t0, t1 = out[lo]["e"], out[hi]["s"]
        weights = [max(len(a[i][0]), 1) for i in gap]
        total = sum(weights)
        c = t0
        for i, w in zip(gap, weights):
            step = (t1 - t0) * w / total
            out[i]["s"], out[i]["e"] = round(c), round(c + step)
            c += step
    return out, len(pairs) / max(len(a), 1)


def main() -> None:
    api, local, dest = sys.argv[1:4]
    o = json.loads(Path(api).read_text())
    l = json.loads(Path(local).read_text())
    o["words"], m = retime(o["words"], l["words"])
    Path(dest).parent.mkdir(parents=True, exist_ok=True)
    Path(dest).write_text(json.dumps(o))
    print(f"{dest}: {m * 100:.1f}% of API words matched a local word")


if __name__ == "__main__":
    main()
