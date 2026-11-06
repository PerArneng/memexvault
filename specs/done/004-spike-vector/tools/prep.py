# /// script
# requires-python = ">=3.12"
# dependencies = ["pyarrow"]
# ///
"""Spec 004: paragraphs from four Wikipedias, shuffled. Passages come from one set of articles, queries from the first sentence of others.
Usage: prep.py <corpus-dir> <n-passages> <n-queries>. Writes passages.txt and queries.txt, one line each."""
import pathlib, random, re, sys
import pyarrow.parquet as pq
root, N, NQ = pathlib.Path(sys.argv[1]), int(sys.argv[2]), int(sys.argv[3])
rng = random.Random(4)
P, Q = [], []
for lang in ("en", "sv", "de", "es"):
    texts = pq.read_table(root / f"{lang}.parquet", columns=["text"]).column("text").to_pylist(); rng.shuffle(texts)
    got = 0; want = N // 4
    for i, t in enumerate(texts):
        paras = [p.strip() for p in t.split("\n") if len(p.split()) >= 30 and len(p.split()) <= 400]
        if not paras: continue
        if len(Q) < (NQ // 4) * (("en", "sv", "de", "es").index(lang) + 1):
            s = re.split(r"(?<=[.!?])\s", paras[0])[0]
            if 4 <= len(s.split()) <= 40: Q.append(s)
            continue
        for p in paras[:4]:
            if got < want: P.append(p); got += 1
        if got >= want: break
    print(lang, "passages", got, "of", want, "queries", len(Q), flush=True)
rng.shuffle(P); rng.shuffle(Q)
(root / "passages.txt").write_text("\n".join(P) + "\n"); (root / "queries.txt").write_text("\n".join(Q) + "\n")
print(len(P), len(Q), "mean words", sum(len(p.split()) for p in P) / len(P))
