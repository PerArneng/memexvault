# /// script
# requires-python = ">=3.12"
# ///
"""Spec 002: rescore saved word output, dropping words whose centre is outside the displayed page, as memex-pdf will.
Reports F1, IoU, x-only IoU (position along the line) and the mismatched words. Usage: rescore.py corpus out"""
import collections, json, pathlib, re, statistics, subprocess, sys, unicodedata, xml.etree.ElementTree as ET
corpus, out = map(pathlib.Path, sys.argv[1:3])
def norm(t): return re.sub(r"\s+", "", unicodedata.normalize("NFKC", t).replace("­", ""))
def truth(pdf):
    x = subprocess.run(["pdftotext", "-bbox", "-cropbox", str(pdf), "-"], capture_output=True, text=True, errors="replace").stdout
    x = re.sub(r"<!DOCTYPE[^>]*>", "", x).replace('xmlns="http://www.w3.org/1999/xhtml"', "")
    pages = []
    for p in ET.fromstring(x).iter("page"):
        pages.append((*size(pdf, len(pages) + 1, float(p.get("width")), float(p.get("height"))), [dict(t=w.text or "", x0=float(w.get("xMin")), y0=float(w.get("yMin")), x1=float(w.get("xMax")), y1=float(w.get("yMax"))) for w in p.iter("word")]))
    return pages
def size(pdf, n, w, h):
    # poppler reports the unrotated size in -bbox, so swap it for pages turned by 90 or 270 degrees.
    i = subprocess.run(["pdfinfo", "-f", str(n), "-l", str(n), str(pdf)], capture_output=True, text=True, errors="replace").stdout
    m = re.search(r"Page\s+\d+ rot:\s+(\d+)", i)
    return (h, w) if m and int(m[1]) % 180 == 90 else (w, h)
def iou(a, b):
    ix, iy = min(a["x1"], b["x1"]) - max(a["x0"], b["x0"]), min(a["y1"], b["y1"]) - max(a["y0"], b["y0"])
    if ix <= 0 or iy <= 0: return 0.0
    i = ix * iy; u = (a["x1"] - a["x0"]) * (a["y1"] - a["y0"]) + (b["x1"] - b["x0"]) * (b["y1"] - b["y0"]) - i
    return i / u if u > 0 else 0.0
total = {}
for cand in ("oxide", "pdfium"):
    tp = fp = fn = 0; file_iou = {}; all_iou = []; miss = collections.Counter(); extra = collections.Counter(); rows = {}
    for pdf in sorted(corpus.rglob("*.pdf")):
        v = json.loads((out / "words" / f"{cand}-{pdf.stem}.json").read_text())["pages"]
        a = b = c = 0; ious = []
        for i, (W, H, gt) in enumerate(truth(pdf)):
            by = collections.defaultdict(list)
            for w in (v[i]["words"] if i < len(v) else []):
                cx, cy = (w["x0"] + w["x1"]) / 2, (w["y0"] + w["y1"]) / 2
                if 1 <= w["x0"] and w["x1"] <= W - 1 and 1 <= w["y0"] and w["y1"] <= H - 1 and norm(w["t"]): by[norm(w["t"])].append(w)
            for g in gt:
                k = norm(g["t"])
                # Words cut by the crop edge: poppler reports the visible part, the candidates the whole word. Excluded by hand check.
                if not k or g["x0"] < 1 or g["y0"] < 1 or g["x1"] > W - 1 or g["y1"] > H - 1: continue
                if by[k]:
                    best = max(by[k], key=lambda w: iou(g, w)); by[k].remove(best); ious.append(iou(g, best)); a += 1
                else:
                    c += 1; miss[k] += 1
            for k, l in by.items():
                b += len(l); extra[k] += len(l)
        p = a / (a + b) if a + b else 1; r = a / (a + c) if a + c else 1
        rows[pdf.stem] = (2 * p * r / (p + r) if p + r else 0, statistics.median(ious) if ious else None, a + c)
        tp += a; fp += b; fn += c; all_iou += ious
    P, R = tp / (tp + fp), tp / (tp + fn)
    print(f"\n{cand}: micro F1={2*P*R/(P+R):.4f} P={P:.4f} R={R:.4f} word median IoU={statistics.median(all_iou):.3f} share IoU>=0.5={sum(x>=.5 for x in all_iou)/len(all_iou):.3f}")
    print(f" | file | words | F1 | median IoU |")
    print(" |---|---|---|---|")
    for k, v in rows.items():
        print(f" | {k} | {v[2]} | {v[0]:.3f} | {'-' if v[1] is None else format(v[1], '.2f')} |")
    print(" files F1<0.98:", {k: round(v[0], 3) for k, v in rows.items() if v[0] < .98 and v[2]})
    print(" files IoU<0.8:", {k: round(v[1], 2) for k, v in rows.items() if v[1] is not None and v[1] < .8})
    print(" missing:", miss.most_common(12)); print(" extra:", extra.most_common(12))
