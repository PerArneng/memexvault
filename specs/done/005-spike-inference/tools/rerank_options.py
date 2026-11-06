# /// script
# requires-python = ">=3.12"
# dependencies = ["numpy"]
# ///
"""Spec 005: can any reranker meet 250 ms? Tries CoreML, shorter inputs and fewer candidates over the chosen embedder's (e5s) results. Usage: rerank_options.py <bench-binary> <root>."""
import json, math, os, pathlib, subprocess, sys
import numpy as np
BIN, root = sys.argv[1], pathlib.Path(sys.argv[2]); out = root / "out"; M = root / "models"
items = json.loads((root / "corpus/wiki.json").read_text()); n = len(items); S = 40
P = {l: np.fromfile(out / f"e5s_p_{l}.f32", dtype=np.float32).reshape(n, -1) for l in ("en", "sv", "de", "es")}
Q = {l: np.fromfile(out / f"e5s_q_{l}.f32", dtype=np.float32).reshape(n, -1) for l in ("en", "sv", "de", "es")}
def ndcg(r): return float(np.mean([1 / math.log2(x + 2) if x < 10 else 0 for x in r]))
PAIRS = [("sv", "sv"), ("de", "de"), ("sv", "en")]
RUNS = [("mmarco-q8", "mmarco", "model_qint8_arm64.onnx", "cpu", 128, 50), ("mmarco-q8", "mmarco", "model_qint8_arm64.onnx", "cpu", 128, 20),
        ("mmarco-f32", "mmarco", "model.onnx", "coreml", 256, 50), 
        ("bgem3-q8", "bgem3", "model.onnx", "cpu", 128, 20), ("bgem3-q8", "bgem3", "model.onnx", "cpu", 128, 10), ("bgem3-q8", "bgem3", "model.onnx", "cpu", 64, 10)]
for name, d, f, ep, ml, k in RUNS:
    row = {}; p95 = []; S = 6 if ep == "coreml" else 40
    for a, b in PAIRS:
        top = np.argsort(-(Q[a][:S] @ P[b].T), axis=1)[:, :k]; fn = out / "ro.jsonl"
        fn.write_text("\n".join(json.dumps(dict(q=items[i]["q"][a], docs=[items[j]["p"][b] for j in top[i]]), ensure_ascii=False) for i in range(S)) + "\n")
        r = subprocess.run([BIN, "rerank", str(M / d), f, "8", ep, str(fn), str(out / "ro_out.jsonl")], capture_output=True, text=True, env={**os.environ, "MAX_LEN": str(ml)})
        if r.returncode: print(name, ep, "failed", r.stderr[-300:], flush=True); break
        o = json.loads(r.stdout.strip().splitlines()[-1]); p95.append(o["p95_ms"])
        sc = [json.loads(l)["scores"] for l in (out / "ro_out.jsonl").read_text().splitlines()]; bef, aft = [], []
        for i in range(S):
            pos = np.where(top[i] == i)[0]
            if len(pos): bef.append(int(pos[0])); aft.append(int(np.where(np.argsort(-np.array(sc[i])) == pos[0])[0][0]))
            else: bef.append(99); aft.append(99)
        row[f"{a}->{b}"] = (round(ndcg(bef), 3), round(ndcg(aft), 3))
    print(name, ep, "len", ml, "k", k, "p95", [round(x) for x in p95], row, flush=True)
