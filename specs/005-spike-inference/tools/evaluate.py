# /// script
# requires-python = ">=3.12"
# dependencies = ["numpy"]
# ///
"""Spec 005: quality and speed for each embedding model and reranker. Usage: evaluate.py <bench-binary> <root>. Run models in series so timings don't overlap."""
import json, math, pathlib, subprocess, sys
import numpy as np
BIN, root = sys.argv[1], pathlib.Path(sys.argv[2]); LANGS = ["en", "sv", "de", "es"]
out = root / "out"; out.mkdir(exist_ok=True); M = root / "models"
items = json.loads((root / "corpus/wiki.json").read_text()); n = len(items); print("items", n)
for l in LANGS:
    (out / f"p_{l}.txt").write_text("\n".join(i["p"][l] for i in items) + "\n")
    (out / f"q_{l}.txt").write_text("\n".join(i["q"][l] for i in items) + "\n")
def bench(*a):
    r = subprocess.run([BIN, *map(str, a)], capture_output=True, text=True)
    if r.returncode: raise SystemExit(r.stderr[-500:])
    return json.loads(r.stdout.strip().splitlines()[-1])
def rss_bench(*a):
    r = subprocess.run(["/usr/bin/time", "-l", BIN, *map(str, a)], capture_output=True, text=True)
    if r.returncode: raise SystemExit(r.stderr[-500:])
    rss = next(int(x.split()[0]) for x in r.stderr.splitlines() if "maximum resident" in x)
    return json.loads(r.stdout.strip().splitlines()[-1]), rss / 1e9
def ndcg(ranks):
    return float(np.mean([1 / math.log2(r + 2) if r < 10 else 0 for r in ranks]))
EMB = [("mini-q8", "mini", "model_qint8_arm64.onnx", "", ""), ("mini-f32", "mini", "model.onnx", "", ""), ("e5s", "e5s", "model.onnx", "query: ", "passage: "), ("e5s-q8", "e5s", "model_q8.onnx", "query: ", "passage: "), ("e5b", "e5b", "model.onnx", "query: ", "passage: ")]
res = {}
vec = {}
for name, d, f, qp, pp in EMB:
    if not (M / d / f).exists(): print("skip", name); continue
    lat, rss = rss_bench("latency", M / d, f, 4, "cpu", qp, out / "q_en.txt")
    lat2 = bench("latency", M / d, f, 1, "cpu", qp, out / "q_sv.txt")
    speed = None; P, Q = {}, {}
    for l in LANGS:
        o = bench("embed", M / d, f, 8, "cpu", pp, out / f"p_{l}.txt", out / f"{name}_p_{l}.f32"); speed = o
        bench("embed", M / d, f, 8, "cpu", qp, out / f"q_{l}.txt", out / f"{name}_q_{l}.f32")
        P[l] = np.fromfile(out / f"{name}_p_{l}.f32", dtype=np.float32).reshape(n, -1); Q[l] = np.fromfile(out / f"{name}_q_{l}.f32", dtype=np.float32).reshape(n, -1)
    table = {}
    for a in LANGS:
        for b in LANGS:
            s = Q[a] @ P[b].T; order = np.argsort(-s, axis=1); ranks = [int(np.where(order[i] == i)[0][0]) for i in range(n)]; table[f"{a}->{b}"] = ndcg(ranks)
    same = np.mean([table[f"{l}->{l}"] for l in LANGS]); cross = np.mean([v for k, v in table.items() if k[:2] != k[-2:]])
    res[name] = dict(dim=int(speed["dim"]), q_p50_4t=lat["p50_ms"], q_p95_4t=lat["p95_ms"], q_p95_1t=lat2["p95_ms"], passages_per_s=speed["per_s"], rss_gb=rss, load_s=lat["load_s"], size_mb=(M / d / f).stat().st_size / 1e6, same=same, cross=cross, table=table)
    vec[name] = (P, Q); print(name, {k: (round(v, 3) if isinstance(v, float) else v) for k, v in res[name].items() if k != "table"}, flush=True); print("  ", {k: round(v, 3) for k, v in table.items() if k[:2] == k[-2:]}, flush=True)
# Rerankers over the top 50 from the best embedder, same-language and cross-language.
base = max(res, key=lambda k: res[k]["same"] + res[k]["cross"]); P, Q = vec[base]; print("rerank base", base)
SAMPLE = 200
for name, d, f in [("mmarco-q8", "mmarco", "model_qint8_arm64.onnx"), ("mmarco-f32", "mmarco", "model.onnx"), ("bgem3-q8", "bgem3", "model.onnx")]:
    agg = {}
    for ep in ("cpu",):
        lat = None
        for a, b in [("sv", "sv"), ("de", "de"), ("es", "es"), ("en", "en"), ("sv", "en"), ("en", "sv"), ("de", "es"), ("es", "de")]:
            s = Q[a][:SAMPLE] @ P[b].T; top = np.argsort(-s, axis=1)[:, :50]
            fn = out / f"rr_{a}_{b}.jsonl"; docs = [i["p"][b] for i in items]; qs = [i["q"][a] for i in items[:SAMPLE]]
            fn.write_text("\n".join(json.dumps(dict(q=qs[i], docs=[docs[j] for j in top[i]]), ensure_ascii=False) for i in range(SAMPLE)) + "\n")
            o, rss = rss_bench("rerank", M / d, f, 8, ep, fn, out / "rr_out.jsonl")
            scores = [json.loads(l)["scores"] for l in (out / "rr_out.jsonl").read_text().splitlines()]
            before, after = [], []
            for i in range(SAMPLE):
                pos = np.where(top[i] == i)[0]; before.append(int(pos[0]) if len(pos) else 99)
                if len(pos):
                    rk = np.argsort(-np.array(scores[i])); after.append(int(np.where(rk == pos[0])[0][0]))
                else: after.append(99)
            agg[f"{a}->{b}"] = (round(ndcg(before), 3), round(ndcg(after), 3)); lat = o
        print(name, "p50", round(lat["p50_ms"]), "p95", round(lat["p95_ms"]), "rss", round(rss, 2), "size_mb", round((M / d / f).stat().st_size / 1e6), "before->after", agg, flush=True)
        res[name] = dict(p50=lat["p50_ms"], p95=lat["p95_ms"], rss=rss, agg=agg)
(out / "results.json").write_text(json.dumps(res, indent=1))
print("done")
