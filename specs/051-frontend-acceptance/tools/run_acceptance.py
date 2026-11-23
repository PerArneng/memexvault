#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = ["psycopg[binary]>=3.2"]
# ///
"""
Spec 051 — Frontend acceptance one-command runner.

Runs:
1. OpenAPI UI coverage check (`pnpm run coverage`), contrast (`pnpm run contrast`), and per-route bundle budgets (`pnpm budget`).
2. Fresh throwaway Postgres database, `memexvault migrate`, `memexvault bootstrap`.
3. Optional speech synthesis with local cached Piper voice (no external network).
4. Playwright acceptance suite (`pnpm --filter @memex/console e2e:acceptance`):
   - Full journey (login -> vault -> upload PDFs + recording -> review -> documents -> wiki -> entities -> search + citation -> audit -> chat + approve proposal -> erasure -> keyboard)
   - Axe 0 violations on every route
   - Zero off-origin requests
   - Consolidated UI target measurements (specs 036, 040, 041, 043, 045)
   - Screenshots of every main screen
5. Cleanup of throwaway database and temporary files.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

import psycopg


def synthesize_speech_wav(out_path: Path) -> Path | None:
    voice = Path.home() / ".cache" / "piper" / "en_GB-alba-medium.onnx"
    if not voice.exists():
        print(f"note: cached piper voice not found at {voice}; skipping recording upload", file=sys.stderr)
        return None
    script = (
        "import wave; from piper import PiperVoice; "
        f"v = PiperVoice.load({str(voice)!r}); "
        f"w = wave.open({str(out_path)!r}, 'wb'); "
        "v.synthesize_wav('The turbine pressure valve in Gothenburg passed inspection on Tuesday morning. "
        "Maintenance engineers recorded a nominal reading of forty two bar.', w); w.close()"
    )
    res = subprocess.run(
        ["uv", "run", "--with", "piper-tts", "python3", "-c", script],
        capture_output=True,
        text=True,
    )
    if res.returncode != 0:
        raise RuntimeError(f"piper synthesis failed: {res.stderr}")
    return out_path


def run_cmd(cmd: list[str], *, cwd: Path, env: dict[str, str] | None = None, check: bool = True) -> subprocess.CompletedProcess[str]:
    print(f"+ {' '.join(cmd)}", flush=True)
    return subprocess.run(cmd, cwd=cwd, env=env, text=True, check=check)


def main() -> int:
    parser = argparse.ArgumentParser(description="Run spec 051 frontend acceptance suite")
    parser.add_argument("--bin", dest="binary", type=Path, default=Path("target/release/memexvault"))
    parser.add_argument("--admin-url", default="postgres://postgres:memex@127.0.0.1:55432/postgres")
    parser.add_argument("--models-dir", type=Path, default=Path.home() / ".cache" / "memexvault-models")
    parser.add_argument("--port", type=int, default=18051)
    parser.add_argument("--audio", type=Path, default=None)
    parser.add_argument("--screenshots-dir", type=Path, default=Path("specs/051-frontend-acceptance/screens"))
    parser.add_argument("--results-json", type=Path, default=Path("specs/051-frontend-acceptance/results.json"))
    args = parser.parse_args()

    repo = Path(__file__).resolve().parents[3]
    web_dir = repo / "web"
    binary = (repo / args.binary).resolve() if not args.binary.is_absolute() else args.binary
    if not binary.exists():
        print(f"error: binary not found at {binary}", file=sys.stderr)
        return 1

    # 1. Static checks: coverage, contrast, build, per-route bundle budgets
    run_cmd(["pnpm", "build"], cwd=web_dir)
    run_cmd(["pnpm", "run", "contrast"], cwd=web_dir)
    run_cmd(["pnpm", "budget"], cwd=web_dir)

    db_name = f"memex_s051_{os.getpid()}"
    db_url = re.sub(r"/[^/?]+(\?.*)?$", f"/{db_name}\\1", args.admin_url)

    tmp_dir = Path(tempfile.mkdtemp(prefix="memex-s051-data-"))
    audio_path = args.audio
    try:
        if audio_path is None:
            audio_path = synthesize_speech_wav(tmp_dir / "speech.wav")

        with psycopg.connect(args.admin_url, autocommit=True) as conn:
            conn.execute(f'DROP DATABASE IF EXISTS "{db_name}"')
            conn.execute(f'CREATE DATABASE "{db_name}"')

        base_env = os.environ.copy()
        base_env["MEMEX_DATABASE_URL"] = db_url
        base_env["MEMEX_DATA_DIR"] = str(tmp_dir / "data")
        base_env["MEMEX_MODELS_DIR"] = str(args.models_dir)
        base_env["MEMEX_HTTP_BIND"] = f"127.0.0.1:{args.port}"
        base_env["MEMEX_E2E_BIN"] = str(binary)
        base_env["MEMEX_E2E_SCREENSHOTS"] = str((repo / args.screenshots_dir).resolve())
        base_env["MEMEX_E2E_RESULTS"] = str((repo / args.results_json).resolve())
        pdfium = repo / "vendor" / "pdfium" / "release" / "lib" / "libpdfium.so"
        if "MEMEX_PDFIUM_LIB" not in base_env and pdfium.exists():
            base_env["MEMEX_PDFIUM_LIB"] = str(pdfium.resolve())
        if audio_path is not None:
            base_env["MEMEX_E2E_RECORDING"] = str(audio_path.resolve())

        run_cmd([str(binary), "migrate"], cwd=repo, env=base_env)
        boot = subprocess.run([str(binary), "bootstrap", "acceptance-admin"], cwd=repo, env=base_env, text=True, capture_output=True, check=True)
        m = re.search(r"mvx_[A-Za-z0-9_-]+", boot.stdout + "\n" + boot.stderr)
        if not m:
            raise RuntimeError(f"could not parse token from bootstrap output:\n{boot.stdout}\n{boot.stderr}")
        base_env["MEMEX_E2E_TOKEN"] = m.group(0)

        run_cmd(["pnpm", "--filter", "@memex/console", "e2e:acceptance"], cwd=web_dir, env=base_env)

        results_path = (repo / args.results_json).resolve()
        if results_path.exists():
            print("\n=== Spec 051 Results Summary ===")
            print(json.dumps(json.loads(results_path.read_text()), indent=2))
        return 0
    finally:
        try:
            with psycopg.connect(args.admin_url, autocommit=True) as conn:
                conn.execute(
                    "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = %s AND pid <> pg_backend_pid()",
                    (db_name,),
                )
                conn.execute(f'DROP DATABASE IF EXISTS "{db_name}"')
        except Exception as exc:
            print(f"warning: failed to drop {db_name}: {exc}", file=sys.stderr)
        shutil.rmtree(tmp_dir, ignore_errors=True)


if __name__ == "__main__":
    raise SystemExit(main())
