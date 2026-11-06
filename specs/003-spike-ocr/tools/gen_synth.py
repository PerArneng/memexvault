# /// script
# requires-python = ">=3.12"
# dependencies = ["reportlab==4.4.*", "pillow", "numpy"]
# ///
"""Spec 003: render born-digital pages at 150 DPI and degrade them, with exact line text and boxes. Writes corpus/synth/*.png and *.json.
Words come from the proofread Wikisource text, so spelling and accents are real. 70 pages: 18/18/17/17 per language."""
import json, pathlib, random, re, subprocess, tempfile
import numpy as np
from PIL import Image, ImageFilter
from reportlab.lib.pagesizes import A4
from reportlab.pdfbase import pdfmetrics
from reportlab.pdfbase.ttfonts import TTFont
from reportlab.pdfgen import canvas
root = pathlib.Path(__file__).parent.parent; out = root / "corpus/synth"; out.mkdir(parents=True, exist_ok=True)
FONTS = {"Times": "Times New Roman.ttf", "Arial": "Arial.ttf", "Georgia": "Georgia.ttf", "Courier": "Courier New.ttf"}
for n, f in FONTS.items(): pdfmetrics.registerFont(TTFont(n, f"/System/Library/Fonts/Supplemental/{f}"))
DEG = ["clean", "blur", "noise", "skew3", "skew5"]
S = 150 / 72
def words(lang):
    w = []
    for f in (root / "corpus/real" / lang).glob("*.txt"): w += re.findall(r"[^\s]+", f.read_text())
    return [x for x in w if len(x) < 18]
def make(name, lang, font, deg, rng):
    W, H = A4; pool = words(lang); size = rng.choice([10, 11, 12]); lead = size * 1.35
    pdf = out / f"{name}.pdf"; c = canvas.Canvas(str(pdf), pagesize=A4); y = H - 70; lines = []
    while y > 70:
        line = []
        while pdfmetrics.stringWidth(" ".join(line + [pool[len(line) and rng.randrange(len(pool)) or rng.randrange(len(pool))]]), font, size) < W - 150: line.append(rng.choice(pool))
        s = " ".join(line); c.setFont(font, size); c.drawString(75, y, s)
        a, d = pdfmetrics.getAscent(font) * size / 1000, -pdfmetrics.getDescent(font) * size / 1000
        lines.append(dict(t=s, x0=75 * S, y0=(H - y - a) * S, x1=(75 + pdfmetrics.stringWidth(s, font, size)) * S, y1=(H - y + d) * S)); y -= lead
    c.save()
    with tempfile.TemporaryDirectory() as t:
        subprocess.run(["pdftoppm", "-r", "150", "-png", "-singlefile", str(pdf), f"{t}/p"], check=True); im = Image.open(f"{t}/p.png").convert("L"); im.load()
    pdf.unlink()
    if deg == "blur": im = im.filter(ImageFilter.GaussianBlur(1.0))
    if deg == "noise":
        a = np.asarray(im, float) + np.random.default_rng(rng.randrange(1 << 30)).normal(0, 22, im.size[::-1]); im = Image.fromarray(np.clip(a, 0, 255).astype("uint8"))
    if deg.startswith("skew"):
        ang = int(deg[4:]) * rng.choice([-1, 1]); im2 = im.rotate(ang, resample=Image.BICUBIC, fillcolor=255)
        cx, cy = im.size[0] / 2, im.size[1] / 2; r = np.deg2rad(ang); co, si = np.cos(r), np.sin(r)
        for l in lines:
            pts = [(l["x0"], l["y0"]), (l["x1"], l["y0"]), (l["x1"], l["y1"]), (l["x0"], l["y1"])]
            q = [((x - cx) * co + (y - cy) * si + cx, -(x - cx) * si + (y - cy) * co + cy) for x, y in pts]
            l["poly"] = q; l["x0"], l["x1"] = min(p[0] for p in q), max(p[0] for p in q); l["y0"], l["y1"] = min(p[1] for p in q), max(p[1] for p in q)
        im = im2
    im.save(out / f"{name}.png")
    (out / f"{name}.json").write_text(json.dumps(dict(lang=lang, font=font, deg=deg, lines=lines), ensure_ascii=False))
i = 0; fonts = list(FONTS)
for lang, n in [("en", 18), ("sv", 18), ("de", 17), ("es", 17)]:
    rng = random.Random(f"synth-{lang}")
    for k in range(n):
        deg = DEG[k % 5]; font = fonts[(k // 5 + k) % 4]
        make(f"{lang}_{k:02d}_{deg}_{font}", lang, font, deg, rng); i += 1
print("pages", i)
