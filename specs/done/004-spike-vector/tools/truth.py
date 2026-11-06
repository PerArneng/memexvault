# /// script
# requires-python = ">=3.12"
# dependencies = ["numpy"]
# ///
"""Spec 004: exact top 50 by cosine with full-precision floats, per scale and condition. Usage: truth.py <out-dir> <dim>.
Reads p.f32 and q.f32. Row id is the line number. bucket(id) and deleted(id) come from a fixed hash, shared with the bench."""
import pathlib, sys
import numpy as np
out, dim = pathlib.Path(sys.argv[1]), int(sys.argv[2])
P = np.fromfile(out / "p.f32", dtype=np.float32).reshape(-1, dim); Q = np.fromfile(out / "q.f32", dtype=np.float32).reshape(-1, dim)
ids = np.arange(len(P), dtype=np.uint64)
h = (ids * np.uint64(2654435761)) % np.uint64(4294967296)
bucket = (h >> np.uint64(8)) % np.uint64(100)
deleted = ((h >> np.uint64(20)) % np.uint64(5)) == 0
print("rows", len(P), "bucket<1", float((bucket < 1).mean()), "deleted", float(deleted.mean()))
for scale in (50_000, 200_000):
    if scale > len(P): continue
    S = Q @ P[:scale].T
    for pct in (100, 50, 20, 10, 5, 2, 1):
        for dele in (False, True):
            mask = bucket[:scale] < pct
            if dele: mask = mask & ~deleted[:scale]
            s = np.where(mask, S, -2.0)
            top = np.argpartition(-s, 50, axis=1)[:, :50]
            name = f"gt_{scale}_{pct}_{'del' if dele else 'all'}.u32"
            top.astype(np.uint32).tofile(out / name); print(name, flush=True)
