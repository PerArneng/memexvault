# /// script
# requires-python = ">=3.12"
# dependencies = ["rapidfuzz", "numpy"]
# ///
"""Spec 003: score OCR output. Usage: score.py <out-dir> <corpus-dir> <config>...

Synthetic pages: regions match truth lines by best box IoU. CER over matched and unmatched lines, line IoU, accented-letter errors.
Real pages: proofread text aligned with free ends, so running headers and page numbers outside the proofread text aren't errors.
Whitespace, hyphens and quote variants are normalised. Case is kept.
"""
import collections, json, pathlib, re, statistics, sys, unicodedata
import numpy as np
from rapidfuzz.distance import Levenshtein as L
out, corpus = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2])
ACC = set("åäöüßñéáíóúÅÄÖÜÉÁÍÓÚÑ")
def norm(t):
    t = unicodedata.normalize("NFC", t)
    t = t.translate(str.maketrans({"’": "'", "‘": "'", "“": '"', "”": '"', "„": '"', "«": '"', "»": '"', "—": "-", "–": "-", "ſ": "s", "⸗": "-", "¬": "-"}))
    return re.sub(r"[\s\-]+", "", t)
def box(p):
    xs, ys = [q[0] for q in p], [q[1] for q in p]
    return min(xs), min(ys), max(xs), max(ys)
def iou(a, b):
    ix, iy = min(a[2], b[2]) - max(a[0], b[0]), min(a[3], b[3]) - max(a[1], b[1])
    if ix <= 0 or iy <= 0: return 0.0
    i = ix * iy
    return i / ((a[2] - a[0]) * (a[3] - a[1]) + (b[2] - b[0]) * (b[3] - b[1]) - i)
def free_end(truth, ocr):
    """Best edit distance of truth against any substring of ocr, with the matching end."""
    prev = np.zeros(len(ocr) + 1, dtype=np.int32); o = np.frombuffer(ocr.encode("utf-32-le"), dtype=np.uint32)
    for ch in truth:
        sub = prev[:-1] + (o != ord(ch))
        cur = np.empty_like(prev); cur[0] = prev[0] + 1; cur[1:] = np.minimum(sub, prev[1:] + 1)
        # Insertions run left to right, done with a running minimum.
        cur = np.minimum.accumulate(cur - np.arange(len(cur))) + np.arange(len(cur))
        prev = cur
    end = int(np.argmin(prev)); return int(prev[end]), end
def accent_errors(truth, ocr):
    ops = L.opcodes(truth, ocr); total = bad = 0
    for op in ops:
        seg = truth[op.src_start:op.src_end]; n = sum(c in ACC for c in seg); total += n
        if op.tag != "equal": bad += n
    return total, bad
for cfg in sys.argv[3:]:
    syn = collections.defaultdict(lambda: [0, 0]); ious = collections.defaultdict(list); real = collections.defaultdict(lambda: [0, 0]); acc = collections.defaultdict(lambda: [0, 0]); accr = collections.defaultdict(lambda: [0, 0]); per = []; pa = {}
    seconds = []; words = 0; regions = 0
    lines = [json.loads(l) for l in (out / f"ocr-{cfg}.jsonl").read_text().splitlines()]
    for rec in lines:
        if rec.get("summary"): summary = rec; continue
        path = pathlib.Path(rec["path"]); seconds.append(rec["seconds"])
        regs = [(box(r["poly"]), r["t"]) for r in rec["regions"]]; regions += len(regs); words += sum(1 for r in rec["regions"] if r.get("word_boxes"))
        if "synth" in path.parts:
            m = json.loads(path.with_suffix(".json").read_text()); deg, lang = m["deg"], m["lang"]; used = set()
            for l in m["lines"]:
                t = (l["x0"], l["y0"], l["x1"], l["y1"]); best = max(range(len(regs)), key=lambda i: iou(t, regs[i][0]) if i not in used else -1, default=None)
                got = ""; v = 0.0
                if best is not None and best not in used:
                    v = iou(t, regs[best][0])
                    if v > 0.1: got = regs[best][1]; used.add(best)
                nt, ng = norm(l["t"]), norm(got); d = L.distance(nt, ng)
                syn[lang][0] += d; syn[lang][1] += len(nt); syn["deg:" + deg][0] += d; syn["deg:" + deg][1] += len(nt); ious[deg].append(v)
                a, b = accent_errors(nt, ng); acc[lang][0] += a; acc[lang][1] += b
        else:
            lang = path.parent.name; truth = norm(path.with_suffix(".txt").read_text())
            # Reading order: the engine gives none, so try one column and two columns split at the page middle, and keep the better one.
            # Two-column pages are few, and memex-ocr must do this ordering itself (recorded in the Result).
            mid = (min((r[0][0] for r in regs), default=0) + max((r[0][2] for r in regs), default=0)) / 2
            one = sorted(regs, key=lambda r: (round(r[0][1] / 12), r[0][0]))
            full = [r for r in regs if r[0][0] < mid < r[0][2]]; left = [r for r in regs if r[0][2] <= mid]; right = [r for r in regs if r[0][0] >= mid]
            ky = lambda r: r[0][1]
            two = sorted(full, key=ky) + sorted(left, key=ky) + sorted(right, key=ky)
            d, end, ocr = min(((*free_end(truth, norm("".join(r[1] for r in o))), norm("".join(r[1] for r in o))) for o in (one, two)), key=lambda x: x[0])
            real[lang][0] += d; real[lang][1] += len(truth); per.append((round(d / len(truth), 3), path.stem))
            s = ocr[max(0, end - len(truth) - d):end]
            # Trim leftover text before the truth starts, so a stray header doesn't count against accented letters.
            k = truth[:12]; p = s.find(k) if k else -1
            if p > 0: s = s[p:]
            a, b = accent_errors(truth, s); accr[lang][0] += a; accr[lang][1] += b; pa[path.stem] = (b, a)
    f = lambda c: f"{100 * c[0] / max(c[1], 1):.2f}%"
    print(f"\n### {cfg}: {summary['ppm']:.0f} pages/min, regions with word boxes {words}/{regions}")
    print("synthetic CER:", {k: f(v) for k, v in sorted(syn.items())})
    print("real CER:", {k: f(v) for k, v in sorted(real.items())}, "all", f([sum(v[0] for v in real.values()), sum(v[1] for v in real.values())]))
    print("accented error rate synth:", {k: f(v[::-1]) + f" of {v[0]}" for k, v in sorted(acc.items())}); print("accented error rate real:", {k: f(v[::-1]) + f" of {v[0]}" for k, v in sorted(accr.items())})
    combined = {k: f([syn[k][0] + real[k][0], syn[k][1] + real[k][1]]) for k in ("en", "sv", "de", "es")}
    print("combined CER per language:", combined)
    print("accented errors per real page:", {k: v for k, v in sorted(pa.items()) if v[0]})
    print("line IoU median:", {k: round(statistics.median(v), 3) for k, v in sorted(ious.items())}, "all", round(statistics.median(sum(ious.values(), [])), 3))
    print("worst real pages:", sorted(per, reverse=True)[:8])
