#!/usr/bin/env python3
"""Spec 004: one query run, compact output, appended to results.txt. Usage: q.py TABLE SCALE PCT all|del THREADS EF ITER MAXSCAN [WRITER] [ip] [exact]"""
import json, os, subprocess, sys
a = sys.argv[1:]; table, scale, pct, dele, th, ef, it, ms = a[:8]; rest = a[8:]
writer = next((x for x in rest if x.isdigit()), "0")
env = dict(os.environ); kind = "halfvec" if table.startswith("half") else "vector"
if "ip" in rest: env["IP"] = "1"
if "exact" in rest: env["EXTRA_SET"] = "SET enable_indexscan=off;"
cmd = ["bench/target/release/vector-bench", "query", "postgres://postgres:memex@127.0.0.1:55432/memex", table, kind, "out/q.f32", "384", f"out/gt_{scale}_{pct}_{dele}.u32", pct, "1000", th, ef, it, ms, writer]
r = subprocess.run(cmd, env=env, capture_output=True, text=True)
if r.returncode: sys.exit(r.stderr[-400:])
d = json.loads(r.stdout)
line = f"{table:8} {pct:>3}% {dele:3} t{th:>2} ef{ef:>4} {it[:7]:7} ms{ms:>6} {' '.join(rest):12} recall {d['recall']:.3f} p50 {d['p50']:6.2f} p95 {d['p95']:6.2f} p99 {d['p99']:6.2f} qps {d['qps']:6.0f}" + (f" writer {d['writer_per_s']:.0f}/s" if d.get('writer_per_s') else "")
print(line, flush=True); open("results.txt", "a").write(line + "\n")
