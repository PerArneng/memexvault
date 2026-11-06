# /// script
# requires-python = ">=3.12"
# dependencies = ["numpy", "pillow"]
# ///
"""Spec 002: render correlation raw and after a Gaussian blur of about 1/100 inch, which removes hinting and antialiasing differences."""
import pathlib, sys
import numpy as np
from PIL import Image, ImageFilter
Image.MAX_IMAGE_PIXELS = None
out = pathlib.Path(sys.argv[1])
def corr(a, b):
    x, y = np.asarray(a, float).ravel(), np.asarray(b, float).ravel()
    if x.std() == 0 or y.std() == 0: return 1.0 if abs(x - y).mean() < 2 else 0.0
    return float(np.corrcoef(x, y)[0, 1])
for dpi in (150, 300):
    sig = dpi / 100
    for c in ("hayro", "oxide", "pdfium"):
        rows = []
        for d in sorted((out / "render" / c / str(dpi)).iterdir()):
            refs = sorted((out / "ref" / str(dpi) / d.name).glob("*.png"))
            for i, r in enumerate(refs):
                p = d / f"p{i}.png"
                if not p.exists(): rows.append((None, None, d.name, i)); continue
                a, b = Image.open(p).convert("L"), Image.open(r).convert("L")
                if a.size != b.size:
                    if max(abs(a.size[0]-b.size[0]), abs(a.size[1]-b.size[1])) > 2: rows.append((None, None, d.name, i, a.size, b.size)); continue
                    a = a.resize(b.size)
                rows.append((corr(a, b), corr(a.filter(ImageFilter.GaussianBlur(sig)), b.filter(ImageFilter.GaussianBlur(sig))), d.name, i))
        ok = [r for r in rows if r[0] is not None]
        low = sorted((r for r in ok if r[1] < .95), key=lambda r: r[1])
        print(f"{c}@{dpi} pages={len(rows)} raw_mean={np.mean([r[0] for r in ok]):.3f} blur_mean={np.mean([r[1] for r in ok]):.3f} blur_min={min(r[1] for r in ok):.3f} below95={[(r[2], r[3], round(r[1],3)) for r in low]} mismatch={[r[2:] for r in rows if r[0] is None]}")
