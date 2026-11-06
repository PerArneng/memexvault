# /// script
# requires-python = ">=3.12"
# dependencies = ["numpy", "pillow"]
# ///
"""Spec 002: run each candidate on each corpus PDF with a timeout and score it against poppler's words and renders.

Usage: uv run compare.py <bench-binary> <corpus-dir> <out-dir> <PDFIUM_DIR>. Prints markdown tables and writes results.json.
"""
import collections, json, os, pathlib, re, statistics, subprocess, sys, time, unicodedata, xml.etree.ElementTree as ET
import numpy as np
from PIL import Image

Image.MAX_IMAGE_PIXELS = None
BIN, CORPUS, OUT, PDFIUM = map(pathlib.Path, sys.argv[1:5])
ENV = {**os.environ, "PDFIUM_DIR": str(PDFIUM)}
TIMEOUT = 120
CATEGORY = {
 "tracemonkey": "multi-column,ligatures,tables", "two_column_en": "multi-column", "three_column_de": "multi-column",
 "table_sv": "tables", "table_es": "tables", "one_column_es_helv": "plain", "one_column_sv": "plain",
 "cropbox_offset": "cropbox", "rotate90": "rotation", "rotate180_crop": "rotation+crop", "rotate270_crop": "rotation+crop",
 "hello_world_rotated": "rotation", "rotation": "rotation", "rotated": "rotation",
 "encrypted_empty_password": "encrypted", "empty_protected": "encrypted", "copy_paste_ligatures": "ligatures",
 "simpletype3font": "type3", "Type3WordSpacing": "type3", "perf_text_type3_vector": "type3", "perf_text_type3_bitmap": "type3",
 "perf_text_cid_truetype": "cid", "cid_cff": "cid", "arial_unicode_en_cidfont": "cid", "ArabicCIDTrueType": "cid",
 "ocr_layer": "hidden-ocr", "perf_images_scan_gray": "hidden-ocr", "perf_images_scan_bilevel": "hidden-ocr",
 "image_only": "image-only", "scan-bad": "image-only", "mixed_text_image": "mixed",
}

def norm(t):
    t = unicodedata.normalize("NFKC", t).replace("­", "")
    return re.sub(r"\s+", "", t)

def truth(pdf):
    """Poppler words per page, in the crop box, after rotation (pdftotext -bbox with -cropbox)."""
    x = subprocess.run(["pdftotext", "-bbox", "-cropbox", str(pdf), "-"], capture_output=True, text=True, errors="replace").stdout
    x = re.sub(r"<!DOCTYPE[^>]*>", "", x)
    x = x.replace('xmlns="http://www.w3.org/1999/xhtml"', "")
    pages = []
    for p in ET.fromstring(x).iter("page"):
        pages.append([dict(t=w.text or "", x0=float(w.get("xMin")), y0=float(w.get("yMin")), x1=float(w.get("xMax")), y1=float(w.get("yMax"))) for w in p.iter("word")])
    return pages

def iou(a, b):
    ix, iy = min(a["x1"], b["x1"]) - max(a["x0"], b["x0"]), min(a["y1"], b["y1"]) - max(a["y0"], b["y0"])
    if ix <= 0 or iy <= 0: return 0.0
    i = ix * iy
    u = (a["x1"] - a["x0"]) * (a["y1"] - a["y0"]) + (b["x1"] - b["x0"]) * (b["y1"] - b["y0"]) - i
    return i / u if u > 0 else 0.0

def score(gt_pages, cand_pages):
    tp = fp = fn = 0; ious = []
    for i, gt in enumerate(gt_pages):
        cand = cand_pages[i]["words"] if i < len(cand_pages) else []
        by = collections.defaultdict(list)
        for w in cand:
            if norm(w["t"]): by[norm(w["t"])].append(w)
        for g in gt:
            k = norm(g["t"])
            if not k: continue
            if by[k]:
                # Pair with the nearest box of the same text, so repeated words don't produce false mismatches.
                if "x0" in by[k][0]:
                    best = max(by[k], key=lambda c: iou(g, c)); by[k].remove(best); ious.append(iou(g, best))
                else:
                    by[k].pop()
                tp += 1
            else:
                fn += 1
        fp += sum(len(v) for v in by.values())
    p = tp / (tp + fp) if tp + fp else 1.0; r = tp / (tp + fn) if tp + fn else 1.0
    f1 = 2 * p * r / (p + r) if p + r else 0.0
    return dict(f1=f1, median_iou=statistics.median(ious) if ious else None, words=tp + fn)

def run(args, timeout=TIMEOUT):
    t = time.time()
    try:
        r = subprocess.run([str(BIN), *args], capture_output=True, text=True, env=ENV, timeout=timeout, errors="replace")
    except subprocess.TimeoutExpired:
        return None, "hang"
    if r.returncode: return None, "panic: " + (r.stderr.strip().splitlines() or ["?"])[0][:120] if "panicked" in r.stderr else f"exit {r.returncode}"
    return json.loads(r.stdout), None

def correlation(a, b):
    a = Image.open(a).convert("L"); b = Image.open(b).convert("L")
    if a.size != b.size:
        return None if abs(a.size[0] - b.size[0]) > 2 or abs(a.size[1] - b.size[1]) > 2 else correlation_same(a.resize(b.size), b)
    return correlation_same(a, b)

def correlation_same(a, b):
    x, y = np.asarray(a, float).ravel(), np.asarray(b, float).ravel()
    if x.std() == 0 or y.std() == 0: return 1.0 if np.abs(x - y).mean() < 2 else 0.0
    return float(np.corrcoef(x, y)[0, 1])

pdfs = sorted(CORPUS.rglob("*.pdf"))
res = {"words": {}, "render": {}}
for cand in ["oxide", "pdfium", "lopdf"]:
    rows = {}
    for pdf in pdfs:
        v, err = run(["words", cand, str(pdf)])
        gt = truth(pdf)
        if err: rows[pdf.stem] = dict(error=err); continue
        s = score(gt, v["pages"]); s["pages"] = len(v["pages"]); s["gt_pages"] = len(gt); s["seconds"] = v["seconds"]
        (OUT / "words").mkdir(parents=True, exist_ok=True); (OUT / "words" / f"{cand}-{pdf.stem}.json").write_text(json.dumps(v))
        rows[pdf.stem] = s
    res["words"][cand] = rows

for dpi in [150, 300]:
    for pdf in pdfs:
        ref = OUT / "ref" / str(dpi) / pdf.stem; ref.mkdir(parents=True, exist_ok=True)
        subprocess.run(["pdftoppm", "-r", str(dpi), "-cropbox", "-png", "-f", "1", "-l", "5", str(pdf), str(ref / "p")], capture_output=True)
    for cand in ["hayro", "oxide", "pdfium"]:
        rows = {}
        for pdf in pdfs:
            d = OUT / "render" / cand / str(dpi) / pdf.stem
            v, err = run(["render", cand, str(pdf), str(dpi), str(d)])
            if err: rows[pdf.stem] = dict(error=err); continue
            refs = sorted((OUT / "ref" / str(dpi) / pdf.stem).glob("*.png"))
            cors = []
            for i, r in enumerate(refs):
                p = d / f"p{i}.png"
                cors.append(correlation(p, r) if p.exists() else None)
            rows[pdf.stem] = dict(correlation=cors, seconds=v["seconds"], pages=v["pages"])
        res["render"][f"{cand}@{dpi}"] = rows

(OUT / "results.json").write_text(json.dumps(res, indent=1))

print("## Words per file (F1 / median IoU / s)\n\n| file | category | words | " + " | ".join(res["words"]) + " |\n|---|---|---|" + "---|" * len(res["words"]))
for pdf in pdfs:
    row = []
    for c in res["words"]:
        s = res["words"][c][pdf.stem]
        row.append(s["error"] if "error" in s else f"{s['f1']:.3f} / {'-' if s['median_iou'] is None else format(s['median_iou'], '.2f')} / {s['seconds']:.3f}")
    w = next((res['words'][c][pdf.stem].get('words') for c in res['words'] if 'words' in res['words'][c][pdf.stem]), '?')
    print(f"| {pdf.stem} | {CATEGORY.get(pdf.stem, '?')} | {w} | " + " | ".join(row) + " |")
print("\n## Words, overall\n\n| candidate | micro F1 | file-median F1 | worst F1 | median of file IoU | min file IoU | failures | pages/s |\n|---|---|---|---|---|---|---|---|")
for c, rows in res["words"].items():
    ok = [r for r in rows.values() if "error" not in r and r["words"]]
    wt = sum(r["words"] for r in ok)
    micro = sum(r["f1"] * r["words"] for r in ok) / wt if wt else 0
    ious = [r["median_iou"] for r in ok if r["median_iou"] is not None]
    pages = sum(r["pages"] for r in rows.values() if "error" not in r); secs = sum(r["seconds"] for r in rows.values() if "error" not in r)
    fails = [k for k, r in rows.items() if "error" in r]
    print(f"| {c} | {micro:.3f} | {statistics.median(r['f1'] for r in ok):.3f} | {min(r['f1'] for r in ok):.3f} | {statistics.median(ious) if ious else float('nan'):.3f} | {min(ious) if ious else float('nan'):.3f} | {fails or 0} | {pages / secs:.0f} |")
print("\n## Render correlation with pdftoppm\n\n| candidate@dpi | pages | mean | min | below 0.95 | size mismatch | failures |\n|---|---|---|---|---|---|---|")
for c, rows in res["render"].items():
    all_c = [(k, x) for k, r in rows.items() if "error" not in r for x in r["correlation"]]
    vals = [x for _, x in all_c if x is not None]
    low = sorted({k for k, x in all_c if x is not None and x < .95})
    mis = sorted({k for k, x in all_c if x is None})
    fails = {k: r["error"] for k, r in rows.items() if "error" in r}
    print(f"| {c} | {len(all_c)} | {sum(vals)/len(vals):.3f} | {min(vals):.3f} | {low} | {mis} | {fails or 0} |")
