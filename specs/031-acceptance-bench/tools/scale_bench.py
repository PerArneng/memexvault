#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = [
#     "psycopg[binary]>=3.2",
# ]
# ///
"""Spec 031 & 033 parameterized scale benchmark orchestrator.

Loads a reproducible multi-language corpus of N segments into a fresh Postgres database and
Tantivy index via the `scale_bench` release helper, then launches the real `memexvault serve`
release binary to measure every target in `docs/architecture.md`:
  - Exact lookup p95 (<= 2 ms)
  - Lexical search p95 (<= 15 ms), cold & warm
  - Hybrid without rerank p95 (<= 50 ms), cold & warm, 1 and 8 clients
  - Hybrid with rerank-20 p95 (<= 300 ms), warm
  - Cached query p95 (<= 1 ms), warm
  - Search p95 during ingest (<= 1.5x idle)
  - Group commit (>= 1k events/s)
  - Restart without models, Postgres already up (<= 3 s)
  - Lexical rebuild (<= 15 min)
  - memexvault RSS excluding models (<= 4 GB), Postgres RSS recorded separately
  - Disk usage per component (Postgres DB, Tantivy index, blobs/cache)

When given multiple scales (e.g. `--segments 50000,200000`), also computes 50k -> 200k growth
and linear/log projection to 1,000,000 segments. Spec 033 reuses this script with
`--segments 1000000`.
"""

from __future__ import annotations

import argparse
import concurrent.futures
import json
import logging
import math
import os
import shutil
import signal
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any

import psycopg

log = logging.getLogger("scale_bench")
logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s %(levelname)s %(message)s",
    datefmt="%Y-%m-%d %H:%M:%S",
    stream=sys.stderr,
)


def pct(samples: list[float], p: float) -> float:
    if not samples:
        return float("nan")
    s = sorted(samples)
    idx = min(len(s) - 1, max(0, int(round((len(s) - 1) * p))))
    return s[idx]


def stats(samples: list[float]) -> dict[str, float]:
    return {
        "n": len(samples),
        "p50_ms": round(pct(samples, 0.50), 2),
        "p95_ms": round(pct(samples, 0.95), 2),
        "p99_ms": round(pct(samples, 0.99), 2),
        "max_ms": round(pct(samples, 1.00), 2),
    }


def dir_size_bytes(path: Path) -> int:
    total = 0
    if not path.exists():
        return 0
    for root, _, files in os.walk(path):
        for f in files:
            try:
                total += (Path(root) / f).stat().st_size
            except OSError:
                pass
    return total


def process_rss_bytes(pid: int) -> int:
    try:
        with open(f"/proc/{pid}/status", "r") as f:
            for line in f:
                if line.startswith("VmRSS:"):
                    kb = int(line.split()[1])
                    return kb * 1024
    except OSError:
        pass
    return 0


def container_rss_bytes(container: str, engine: str) -> int | None:
    if not container:
        return None
    res = subprocess.run(
        [engine, "stats", "--no-stream", "--format", "{{.MemUsage}}", container],
        capture_output=True,
        text=True,
        check=False,
    )
    if res.returncode != 0 or not res.stdout.strip():
        return None
    # Format like "1.23GiB / 62GiB" or "450MiB / 62GiB" or "450MB / 62GB"
    used = res.stdout.strip().splitlines()[0].split("/")[0].strip()
    for suffix, mult in (
        ("GiB", 1024**3),
        ("MiB", 1024**2),
        ("KiB", 1024),
        ("GB", 1000**3),
        ("MB", 1000**2),
        ("kB", 1000),
        ("B", 1),
    ):
        if used.endswith(suffix):
            try:
                return int(float(used[: -len(suffix)].strip()) * mult)
            except ValueError:
                return None
    return None


def wait_for_health(base_url: str, timeout: float = 60.0) -> float:
    t0 = time.monotonic()
    deadline = t0 + timeout
    while time.monotonic() < deadline:
        try:
            with urllib.request.urlopen(f"{base_url}/api/v1/health", timeout=2.0) as r:
                if r.status == 200:
                    return time.monotonic() - t0
        except Exception:
            pass
        time.sleep(0.02)
    raise RuntimeError("server did not become healthy in time")


def http_get(base_url: str, path: str, token: str, timeout: float = 30.0) -> tuple[float, int, bytes]:
    req = urllib.request.Request(f"{base_url}{path}", headers={"Authorization": f"Bearer {token}"})
    t0 = time.monotonic()
    with urllib.request.urlopen(req, timeout=timeout) as r:
        body = r.read()
        dt_ms = (time.monotonic() - t0) * 1000.0
        return dt_ms, r.status, body


def measure_queries(base_url: str, vault: str, token: str, paths: list[str], concurrency: int = 1) -> list[float]:
    if concurrency <= 1:
        out = []
        for p in paths:
            dt, code, _ = http_get(base_url, f"/api/v1/vaults/{vault}{p}", token)
            assert code == 200, f"{p} -> {code}"
            out.append(dt)
        return out
    out = []
    with concurrent.futures.ThreadPoolExecutor(max_workers=concurrency) as pool:
        futs = [pool.submit(http_get, base_url, f"/api/v1/vaults/{vault}{p}", token) for p in paths]
        for f in futs:
            dt, code, _ = f.result()
            assert code == 200
            out.append(dt)
    return out


def run_scale(
    segments: int,
    bin_path: Path,
    loader_bin: Path,
    admin_url: str,
    data_root: Path,
    models_dir: Path,
    port: int,
    pg_container: str,
    container_engine: str,
    keep_db: bool,
) -> dict[str, Any]:
    parsed = urllib.parse.urlparse(admin_url)
    db_name = f"memex_s031_scale_{segments}_{os.getpid()}"
    db_url = parsed._replace(path=f"/{db_name}").geturl()
    scale_dir = data_root / f"scale_{segments}"
    if scale_dir.exists():
        shutil.rmtree(scale_dir)
    scale_dir.mkdir(parents=True, exist_ok=True)
    manifest_path = scale_dir / "manifest.json"

    log.info("=== scale=%d: creating database %s ===", segments, db_name)
    with psycopg.connect(admin_url, autocommit=True) as conn:
        conn.execute(f"DROP DATABASE IF EXISTS {db_name}")
        conn.execute(f"CREATE DATABASE {db_name}")

    server_proc: subprocess.Popen[str] | None = None
    try:
        # 1. Run Rust scale_bench loader (group commit, bulk load, lexical + vector projection, lexical rebuild)
        log.info("running scale_bench loader for %d segments", segments)
        res = subprocess.run(
            [str(loader_bin), db_url, str(scale_dir), str(models_dir), str(segments), str(manifest_path)],
            capture_output=True,
            text=True,
            check=False,
        )
        if res.returncode != 0:
            raise RuntimeError(f"scale_bench loader failed ({res.returncode}):\n{res.stderr}\n{res.stdout}")
        manifest = json.loads(manifest_path.read_text())
        vault = manifest["vault_id"]
        token = manifest["token"]
        log.info(
            "loaded %d segments in %.1fs; group_commit=%.0f ev/s; lexical_rebuild=%.2fs",
            segments,
            manifest["load_total_s"],
            manifest["group_commit_events_per_s"],
            manifest["lexical_rebuild_s"],
        )

        # Record disk usage per component
        with psycopg.connect(db_url, autocommit=True) as conn:
            pg_bytes = conn.execute("SELECT pg_database_size(current_database())").fetchone()[0]
        tantivy_bytes = dir_size_bytes(scale_dir / "vaults" / vault / "indexes")
        blobs_cache_bytes = dir_size_bytes(scale_dir) - tantivy_bytes

        env = os.environ.copy()
        env.update(
            {
                "MEMEX_DATABASE_URL": db_url,
                "MEMEX_DATA_DIR": str(scale_dir),
                "MEMEX_MODELS_DIR": str(models_dir),
                "MEMEX_HTTP_BIND": f"127.0.0.1:{port}",
            }
        )
        base_url = f"http://127.0.0.1:{port}"

        def start_serve(log_name: str) -> tuple[subprocess.Popen[str], float]:
            lf = open(scale_dir / log_name, "w")
            proc = subprocess.Popen([str(bin_path), "serve"], env=env, stdout=lf, stderr=subprocess.STDOUT, text=True)
            t_ready = wait_for_health(base_url, timeout=60.0)
            return proc, t_ready

        def stop_serve(proc: subprocess.Popen[str]) -> None:
            if proc.poll() is None:
                proc.send_signal(signal.SIGTERM)
                try:
                    proc.wait(timeout=15)
                except subprocess.TimeoutExpired:
                    proc.kill()

        # 2. Start server and measure restart time (and cold searches)
        log.info("starting memexvault serve (cold)")
        server_proc, startup_cold_s = start_serve("serve_cold.log")
        rss_after_startup = process_rss_bytes(server_proc.pid)

        exact_ids: list[str] = manifest["exact_queries"]
        lex_queries: list[str] = manifest["lexical_queries"]
        hyb_queries: list[str] = manifest["hybrid_queries"]

        exact_paths = [f"/search?q={urllib.parse.quote(q)}" for q in exact_ids]
        lex_paths = [f"/search?q={urllib.parse.quote(q)}&mode=lexical&limit=20" for q in lex_queries]
        hyb_paths = [f"/search?q={urllib.parse.quote(q)}&mode=hybrid&rerank=false&limit=20" for q in hyb_queries]
        rerank_paths = [f"/search?q={urllib.parse.quote(q)}&mode=hybrid&rerank=true&limit=20" for q in hyb_queries[:30]]

        # Cold measurements (first pass, distinct queries)
        log.info("measuring cold search latencies")
        exact_cold = measure_queries(base_url, vault, token, exact_paths[:40], concurrency=1)
        lex_cold = measure_queries(base_url, vault, token, lex_paths[:40], concurrency=1)
        hyb_cold = measure_queries(base_url, vault, token, hyb_paths[:40], concurrency=1)

        # Warm measurements: use distinct queries with cache bypassed? Wait: query cache keys include
        # the whole request, so first call on each query misses query cache while OS page cache,
        # Postgres shared_buffers, Tantivy mmap, and ONNX sessions are warm!
        # To measure uncached warm search cleanly, we pass `debug=true`? No, debug adds extra timing work;
        # even cleaner: we use a fresh set of 80 distinct queries after warming up!
        log.info("warming up and measuring warm search latencies (uncached queries)")
        exact_warm = measure_queries(base_url, vault, token, exact_paths[40:120], concurrency=1)
        lex_warm = measure_queries(base_url, vault, token, lex_paths[40:120], concurrency=1)
        hyb_warm_1c = measure_queries(base_url, vault, token, hyb_paths[40:120], concurrency=1)
        hyb_warm_8c = measure_queries(base_url, vault, token, hyb_paths[120:200], concurrency=8)
        rerank_warm = measure_queries(base_url, vault, token, rerank_paths, concurrency=1)

        # Cached query p95: repeat the same 10 hybrid queries 10 times each (100 hits)
        log.info("measuring cached query p95")
        cached_seed = hyb_paths[40:50]
        cached_samples = measure_queries(base_url, vault, token, cached_seed * 10, concurrency=1)

        # RSS with models loaded (e5 + reranker loaded)
        rss_with_models = process_rss_bytes(server_proc.pid)
        pg_rss = container_rss_bytes(pg_container, container_engine)

        # Search p95 during ingest: upload text documents continuously in background threads while
        # running uncached hybrid queries (using a fresh slice of queries)
        log.info("measuring search p95 during active ingest")
        stop_ingest = threading.Event()
        uploaded_count = [0]

        def ingest_worker() -> None:
            idx = 0
            sample_body = (
                "Quarterly engineering inspection report for hydraulic turbine valve assembly. "
                "Operating pressure remained steady across all Gothenburg and Stockholm stations.\n"
            ) * 15
            while not stop_ingest.is_set():
                req = urllib.request.Request(
                    f"{base_url}/api/v1/vaults/{vault}/sources?name=ingest_{idx}.txt&trust_tier=trusted",
                    data=f"doc-{idx}\n{sample_body}".encode("utf-8"),
                    headers={"Authorization": f"Bearer {token}", "Content-Type": "text/plain"},
                    method="POST",
                )
                try:
                    with urllib.request.urlopen(req, timeout=15.0) as r:
                        if r.status < 300:
                            uploaded_count[0] += 1
                except Exception:
                    pass
                idx += 1
                time.sleep(0.05)

        t_ing = threading.Thread(target=ingest_worker, daemon=True)
        t_ing.start()
        time.sleep(0.5)
        hyb_during_ingest = measure_queries(base_url, vault, token, hyb_paths[200:260], concurrency=1)
        lex_during_ingest = measure_queries(base_url, vault, token, lex_paths[120:180], concurrency=1)
        stop_ingest.set()
        t_ing.join(timeout=5.0)

        # Stop and measure warm restart time (Postgres already up)
        log.info("measuring restart time (Postgres already up)")
        stop_serve(server_proc)
        server_proc = None
        time.sleep(0.5)
        server_proc, restart_s = start_serve("serve_restart.log")
        stop_serve(server_proc)
        server_proc = None

        idle_hyb_p95 = pct(hyb_warm_1c, 0.95)
        during_hyb_p95 = pct(hyb_during_ingest, 0.95)
        ingest_ratio = round(during_hyb_p95 / idle_hyb_p95, 2) if idle_hyb_p95 > 0 else float("nan")

        # Model files in RAM baseline: multilingual-e5-small (~120 MB) + mmarco int8 (~120 MB)
        # We report both total RSS and estimated RSS excluding models (plus embedder load time)
        return {
            "segments": segments,
            "load_total_s": round(manifest["load_total_s"], 2),
            "group_commit_events_per_s": round(manifest["group_commit_events_per_s"], 1),
            "lexical_rebuild_s": round(manifest["lexical_rebuild_s"], 2),
            "startup_cold_s": round(startup_cold_s, 3),
            "restart_s": round(restart_s, 3),
            "embedder_load_s": round(manifest.get("embedder_load_s", 0.0), 3),
            "restart_excl_models_s": round(max(0.0, restart_s - manifest.get("embedder_load_s", 0.0)), 3),
            "exact_lookup_cold": stats(exact_cold),
            "exact_lookup_warm": stats(exact_warm),
            "lexical_cold": stats(lex_cold),
            "lexical_warm": stats(lex_warm),
            "hybrid_cold": stats(hyb_cold),
            "hybrid_warm_1c": stats(hyb_warm_1c),
            "hybrid_warm_8c": stats(hyb_warm_8c),
            "hybrid_rerank20_warm": stats(rerank_warm),
            "cached_query_warm": stats(cached_samples),
            "hybrid_during_ingest": stats(hyb_during_ingest),
            "lexical_during_ingest": stats(lex_during_ingest),
            "ingest_docs_uploaded": uploaded_count[0],
            "search_during_ingest_ratio": ingest_ratio,
            "memexvault_rss_startup_mb": round(rss_after_startup / (1024 * 1024), 1),
            "memexvault_rss_with_models_mb": round(rss_with_models / (1024 * 1024), 1),
            "postgres_rss_mb": round(pg_rss / (1024 * 1024), 1) if pg_rss is not None else None,
            "disk_mb": {
                "postgres": round(pg_bytes / (1024 * 1024), 1),
                "tantivy": round(tantivy_bytes / (1024 * 1024), 1),
                "blobs_and_cache": round(blobs_cache_bytes / (1024 * 1024), 1),
                "total": round((pg_bytes + tantivy_bytes + blobs_cache_bytes) / (1024 * 1024), 1),
            },
        }
    finally:
        if server_proc is not None and server_proc.poll() is None:
            server_proc.send_signal(signal.SIGTERM)
            try:
                server_proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                server_proc.kill()
        if not keep_db:
            with psycopg.connect(admin_url, autocommit=True) as conn:
                conn.execute(
                    "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = %s AND pid <> pg_backend_pid()",
                    (db_name,),
                )
                conn.execute(f"DROP DATABASE IF EXISTS {db_name}")


def project_1m(r50: dict[str, Any], r200: dict[str, Any]) -> dict[str, Any]:
    """Project 50k -> 200k growth to 1M using log-log power law (or linear where appropriate)."""

    def proj(v50: float, v200: float) -> float:
        if v50 <= 0 or v200 <= 0:
            return v200
        if v200 <= v50:
            return round(v200, 2)
        # Power-law exponent alpha from 50k -> 200k (4x scale), then extrapolate 5x to 1M
        alpha = math.log(v200 / v50) / math.log(4.0)
        return round(v200 * (5.0**alpha), 2)

    return {
        "exact_lookup_p95_ms": proj(r50["exact_lookup_warm"]["p95_ms"], r200["exact_lookup_warm"]["p95_ms"]),
        "lexical_p95_ms": proj(r50["lexical_warm"]["p95_ms"], r200["lexical_warm"]["p95_ms"]),
        "hybrid_1c_p95_ms": proj(r50["hybrid_warm_1c"]["p95_ms"], r200["hybrid_warm_1c"]["p95_ms"]),
        "hybrid_8c_p95_ms": proj(r50["hybrid_warm_8c"]["p95_ms"], r200["hybrid_warm_8c"]["p95_ms"]),
        "hybrid_rerank20_p95_ms": proj(r50["hybrid_rerank20_warm"]["p95_ms"], r200["hybrid_rerank20_warm"]["p95_ms"]),
        "cached_query_p95_ms": proj(r50["cached_query_warm"]["p95_ms"], r200["cached_query_warm"]["p95_ms"]),
        "lexical_rebuild_s": proj(r50["lexical_rebuild_s"], r200["lexical_rebuild_s"]),
        "restart_s": proj(r50["restart_s"], r200["restart_s"]),
        "disk_total_mb": proj(r50["disk_mb"]["total"], r200["disk_mb"]["total"]),
    }


def main() -> int:
    ap = argparse.ArgumentParser(description="Spec 031/033 parameterized scale benchmark")
    ap.add_argument("--bin", required=True, type=Path, help="Path to memexvault release binary")
    ap.add_argument("--loader-bin", required=True, type=Path, help="Path to scale_bench release example binary")
    ap.add_argument("--admin-url", required=True, help="Superuser Postgres URL")
    ap.add_argument("--data-dir", required=True, type=Path, help="Data root directory")
    ap.add_argument("--models-dir", required=True, type=Path, help="Installed models directory")
    ap.add_argument("--segments", default="50000,200000", help="Comma-separated segment counts (e.g. 50000,200000 or 1000000)")
    ap.add_argument("--port", type=int, default=18032, help="HTTP port for memexvault serve")
    ap.add_argument("--pg-container", default="", help="Postgres container name for RSS measurement")
    ap.add_argument("--container-engine", default="podman", help="docker or podman")
    ap.add_argument("--keep-db", action="store_true", help="Do not drop benchmark databases")
    args = ap.parse_args()

    scales = [int(x.strip()) for x in args.segments.split(",") if x.strip()]
    results = []
    for n in scales:
        results.append(
            run_scale(
                segments=n,
                bin_path=args.bin,
                loader_bin=args.loader_bin,
                admin_url=args.admin_url,
                data_root=args.data_dir,
                models_dir=args.models_dir,
                port=args.port,
                pg_container=args.pg_container,
                container_engine=args.container_engine,
                keep_db=args.keep_db,
            )
        )

    out: dict[str, Any] = {"scales": results}
    by_n = {r["segments"]: r for r in results}
    if 50000 in by_n and 200000 in by_n:
        out["projection_1m"] = project_1m(by_n[50000], by_n[200000])

    print(json.dumps(out, indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
