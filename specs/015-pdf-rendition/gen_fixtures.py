# /// script
# requires-python = ">=3.12"
# dependencies = ["pypdf"]
# ///
"""Spec 015: labels 50 anchors with poppler's boxes, and makes the bad-case PDFs. Usage: gen_fixtures.py <fixture-dir>."""
import json, pathlib, random, re, subprocess, sys, xml.etree.ElementTree as ET
from pypdf import PdfReader, PdfWriter
d = pathlib.Path(sys.argv[1]); rng = random.Random(15)
names = ["rotate90", "rotate180_crop", "rotate270_crop", "cropbox_offset", "two_column_en", "three_column_de", "one_column_sv", "table_es"]
labels = []
for name in names:
    x = subprocess.run(["pdftotext", "-bbox", "-cropbox", str(d / f"{name}.pdf"), "-"], capture_output=True, text=True, errors="replace").stdout
    x = re.sub(r"<!DOCTYPE[^>]*>", "", x).replace('xmlns="http://www.w3.org/1999/xhtml"', "")
    rot = int(next(l.split()[-1] for l in subprocess.run(["pdfinfo", str(d / f"{name}.pdf")], capture_output=True, text=True).stdout.splitlines() if "rot" in l.lower()))
    cands = []
    for pi, p in enumerate(ET.fromstring(x).iter("page")):
        pw, ph = float(p.get("width")), float(p.get("height"))
        ws = [(w.text or "", float(w.get("xMin")), float(w.get("yMin")), float(w.get("xMax")), float(w.get("yMax"))) for w in p.iter("word")]
        for i in range(len(ws) - 2):
            tri = ws[i:i + 3]
            if not all(re.fullmatch(r"\w[\w'’.,-]*", t[0]) for t in tri): continue
            # A word at the crop's edge may be cut in two, and spec 002 drops words that cross the crop.
            if any(t[1] < 5 or t[2] < 5 or t[3] > pw - 5 or t[4] > ph - 5 for t in tri): continue
            horizontal = max(abs(t[2] - tri[0][2]) for t in tri) <= 1 and (all(tri[k + 1][1] > tri[k][1] for k in range(2)) or all(tri[k + 1][1] < tri[k][1] for k in range(2)))
            vertical = max(abs(t[1] - tri[0][1]) for t in tri) <= 1 and (all(tri[k + 1][2] > tri[k][2] for k in range(2)) or all(tri[k + 1][2] < tri[k][2] for k in range(2)))
            if not (horizontal or vertical): continue
            cands.append((pi, tri))
    rng.shuffle(cands)
    for pi, tri in cands[:7]:
        labels.append({"file": name, "page": pi, "quote": " ".join(t[0] for t in tri), "x0": min(t[1] for t in tri), "y0": min(t[2] for t in tri), "x1": max(t[3] for t in tri), "y1": max(t[4] for t in tri), "rotation": rot})
import collections
print(len(labels), collections.Counter(l["file"] for l in labels))
rng.shuffle(labels)
want = {}
kept = []
for l in sorted(labels, key=lambda l: l["file"] in ("rotate90", "rotate180_crop", "rotate270_crop", "cropbox_offset"), reverse=True):
    if len(kept) < 50: kept.append(l)
labels = kept
print(collections.Counter(l["file"] for l in labels))
(d / "labels.json").write_text(json.dumps(labels[:50], ensure_ascii=False, indent=0))
w = PdfWriter(); w.append(PdfReader(d / "one_column_sv.pdf")); w.encrypt("secret", algorithm="AES-256"); w.write(d / "password_protected.pdf")
w = PdfWriter(); w.write(d / "zero_pages.pdf")
