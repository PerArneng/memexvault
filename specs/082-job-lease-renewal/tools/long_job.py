#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = [
#     "pydantic>=2.7",
# ]
# ///
"""Run a long PDF through a real memexvault server, optionally killing the server part way.

What it does:
  Migrates and bootstraps an empty database, starts `memexvault serve`, uploads the PDF to a new
  vault and watches its rendition job. With --mode ingest it waits for the job to end. With
  --mode kill it waits until the job's lease has been renewed at least once, kills the server
  with SIGKILL, starts a new one straight away and waits for the job to end. It only ever stops
  servers it started. Spec 082's pass rules for items 1 and 3 are in that spec.

Output:
  stdout  One JSON report of what was measured.
  stderr  Log lines: "YYYY-MM-DD HH:MM LEVEL message key=value ...".
          Levels: DEBUG (only with -v), INFO, WARN, ERROR, FAIL.

Exit codes:
  0  the run finished and the report is printed, whatever the job's outcome
  1  failure (a FAIL line on stderr says why)
  2  invalid command line (printed by argparse)

Side effects:
  Writes to the database and data folder given, so point them at ones of your own. Reads
  lease_until_ms through `docker exec` on --container, and writes nothing that way.

Examples:
  ./long_job.py --mode ingest --bin target/release/memexvault --database-url postgres://postgres:memex@127.0.0.1:55432/memex_s082 \\
      --data-dir target/s082/data --models-dir target/s082/models --port 18082 --pdf target/s082/long.pdf

--help output:
[BEGIN HELP]
  $ long_job.py --help
  usage: long_job.py [-h] [-v] --mode {ingest,kill} --bin BIN --database-url URL
                     --data-dir DIR --models-dir DIR --port PORT --pdf PDF
                     [--container NAME] [--limit-s S]

  Run a long PDF through a real memexvault server, optionally killing the server part way.

  options:
    -h, --help            show this help message and exit
    -v, --verbose         log DEBUG lines too (default: False)
    --mode {ingest,kill}  what to do during the job (default: None)
    --bin BIN             the memexvault binary (default: None)
    --database-url URL    an empty database of your own (default: None)
    --data-dir DIR        a data folder of your own (default: None)
    --models-dir DIR      the installed models (default: None)
    --port PORT           the port to serve on (default: None)
    --pdf PDF             the PDF to upload (default: None)
    --container NAME      the Postgres container, to read the lease (default: memexvault-postgres-1)
    --limit-s S           give up after this long (default: 1800)
[END HELP]
"""

from __future__ import annotations

import argparse
import json
import logging
import os
import re
import signal
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

from pydantic import BaseModel, ConfigDict

log = logging.getLogger("long_job")


# --------------------------------------------------------------------------- logging

logging.addLevelName(logging.WARNING, "WARN")
logging.addLevelName(logging.CRITICAL, "FAIL")

_RESET = "\033[0m"
_KEY_STYLE = "\033[1;97m"
_TIME_STYLE = "\033[2m"
_LEVEL_STYLES = {
    logging.DEBUG: "\033[36m",
    logging.INFO: "\033[32m",
    logging.WARNING: "\033[33m",
    logging.ERROR: "\033[31m",
    logging.CRITICAL: "\033[1;97;41m",
}
_KEY_RE = re.compile(r"(?<![\w.\-/=])([A-Za-z_][\w.\-]*)=")


class LogFormatter(logging.Formatter):
    """Formats `2021-01-12 15:56 INFO message key=value`, colored when writing to a TTY."""

    def __init__(self, color: bool) -> None:
        super().__init__(datefmt="%Y-%m-%d %H:%M")
        self.color = color

    def format(self, record: logging.LogRecord) -> str:
        stamp = self.formatTime(record, self.datefmt)
        level = record.levelname
        message = record.getMessage()
        if self.color:
            stamp = f"{_TIME_STYLE}{stamp}{_RESET}"
            level = f"{_LEVEL_STYLES.get(record.levelno, '')}{level}{_RESET}"
            message = _KEY_RE.sub(rf"{_KEY_STYLE}\1{_RESET}=", message)
        line = f"{stamp} {level} {message}"
        if record.exc_info:
            line += "\n" + self.formatException(record.exc_info)
        return line


def setup_logging(verbose: bool) -> None:
    handler = logging.StreamHandler(sys.stderr)
    color = sys.stderr.isatty() and "NO_COLOR" not in os.environ
    handler.setFormatter(LogFormatter(color))
    root = logging.getLogger()
    root.handlers[:] = [handler]
    root.setLevel(logging.DEBUG if verbose else logging.INFO)


def kv(**fields: object) -> str:
    """Render fields as `key=value` pairs; strings with spaces or quotes are JSON-quoted."""
    return " ".join(f"{key}={_fmt_value(value)}" for key, value in fields.items())


def _fmt_value(value: object) -> str:
    if isinstance(value, Path):
        value = str(value)
    if isinstance(value, str):
        needs_quotes = value == "" or any(c.isspace() or c in "\"'=" for c in value)
        return json.dumps(value, ensure_ascii=False) if needs_quotes else value
    if isinstance(value, float):
        return f"{value:.3f}".rstrip("0").rstrip(".")
    return str(value)


class ScriptFailure(Exception):
    """Raised for an expected, explainable failure. main() logs it as FAIL and exits 1."""


# --------------------------------------------------------------------------- models


class Kill(BaseModel):
    model_config = ConfigDict(frozen=True)

    killed_at_ms: int
    renewals_seen: int
    lease_left_ms: int
    new_server_ready_ms: int


class Report(BaseModel):
    model_config = ConfigDict(frozen=True)

    state: str
    attempts: list[dict[str, object]]
    attempt_seconds: list[float]
    pages: int | None
    total_seconds: float
    load_at_start: str
    load_at_end: str
    kill: Kill | None
    second_attempt_after_kill_ms: int | None


# --------------------------------------------------------------------------- pure core


def attempt_seconds(attempts: list[dict[str, object]]) -> list[float]:
    spans = []
    for a in attempts:
        start, end = a.get("started_at_ms"), a.get("finished_at_ms")
        if isinstance(start, int) and isinstance(end, int):
            spans.append((end - start) / 1000)
    return spans


# --------------------------------------------------------------------------- io edge


def load_average() -> str:
    return os.getloadavg().__repr__()


def call(origin: str, token: str, method: str, path: str, body: bytes | None = None) -> dict[str, object]:
    request = urllib.request.Request(f"{origin}{path}", data=body, method=method, headers={"authorization": f"Bearer {token}"})
    if body is not None and not path.split("?")[0].endswith("/sources"):
        request.add_header("content-type", "application/json")
    with urllib.request.urlopen(request, timeout=300) as response:
        return json.loads(response.read())


def start_server(binary: Path, env: dict[str, str], origin: str, log_to: Path) -> tuple[subprocess.Popen[bytes], int]:
    started = time.monotonic()
    server = subprocess.Popen([str(binary), "serve"], env=env, stdout=log_to.open("ab"), stderr=subprocess.STDOUT)
    while time.monotonic() - started < 300:
        if server.poll() is not None:
            raise ScriptFailure(f"the server exited {kv(code=server.returncode, log=log_to)}")
        try:
            urllib.request.urlopen(f"{origin}/api/v1/health", timeout=5)
            return server, int((time.monotonic() - started) * 1000)
        except (urllib.error.URLError, OSError):
            time.sleep(0.1)
    server.kill()
    raise ScriptFailure("the server didn't start in 300 s")


def lease_until(container: str, database: str, vault: str, job: str) -> int | None:
    query = f"select nspname from pg_namespace where nspname like 'vault\\_{vault}%'"
    run = lambda statement: subprocess.run(  # noqa: E731
        ["docker", "exec", container, "psql", "-U", "postgres", "-d", database, "-Atc", statement], check=True, capture_output=True, text=True
    ).stdout.strip()
    schema = run(query)
    found = run(f"select lease_until_ms from \"{schema}\".jobs where id = '{job}' and state = 'running'")
    return int(found) if found else None


def wait_for_job(origin: str, token: str, vault: str, source: str, deadline: float) -> dict[str, object]:
    while time.monotonic() < deadline:
        items = call(origin, token, "GET", f"/api/v1/vaults/{vault}/jobs?kind=rendition&limit=200")["items"]
        assert isinstance(items, list)
        for job in items:
            if job["input"].get("source_id") == source:
                return job
        time.sleep(0.2)
    raise ScriptFailure("the job never appeared")


def wait_for_end(origin: str, token: str, vault: str, source: str, deadline: float) -> dict[str, object]:
    while time.monotonic() < deadline:
        try:
            job = wait_for_job(origin, token, vault, source, deadline)
        except (urllib.error.URLError, OSError) as e:
            log.warning("can't read the job yet %s", kv(detail=str(e)))
            time.sleep(1)
            continue
        if job["state"] in ("done", "dead", "cancelled"):
            return job
        time.sleep(1)
    raise ScriptFailure("the job didn't end in time")


# --------------------------------------------------------------------------- cli


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=(__doc__ or "").splitlines()[0], formatter_class=argparse.ArgumentDefaultsHelpFormatter)
    parser.add_argument("-v", "--verbose", action="store_true", help="log DEBUG lines too")
    parser.add_argument("--mode", choices=["ingest", "kill"], required=True, help="what to do during the job")
    parser.add_argument("--bin", type=Path, required=True, help="the memexvault binary")
    parser.add_argument("--database-url", required=True, metavar="URL", help="an empty database of your own")
    parser.add_argument("--data-dir", type=Path, required=True, metavar="DIR", help="a data folder of your own")
    parser.add_argument("--models-dir", type=Path, required=True, metavar="DIR", help="the installed models")
    parser.add_argument("--port", type=int, required=True, help="the port to serve on")
    parser.add_argument("--pdf", type=Path, required=True, help="the PDF to upload")
    parser.add_argument("--container", default="memexvault-postgres-1", metavar="NAME", help="the Postgres container, to read the lease")
    parser.add_argument("--limit-s", type=int, default=1800, metavar="S", help="give up after this long")
    return parser.parse_args(argv)


def run(args: argparse.Namespace) -> Report:
    env = {
        **os.environ,
        "MEMEX_DATABASE_URL": args.database_url,
        "MEMEX_DATA_DIR": str(args.data_dir),
        "MEMEX_MODELS_DIR": str(args.models_dir),
        "MEMEX_HTTP_BIND": f"127.0.0.1:{args.port}",
    }
    origin = f"http://127.0.0.1:{args.port}"
    deadline = time.monotonic() + args.limit_s
    args.data_dir.mkdir(parents=True, exist_ok=True)
    server_log = args.data_dir / "server.log"
    subprocess.run([str(args.bin), "migrate"], env=env, check=True, capture_output=True)
    made = subprocess.run([str(args.bin), "bootstrap", "long-job"], env=env, check=True, capture_output=True, text=True).stdout
    token = made.strip().split()[-1]
    server, _ = start_server(args.bin, env, origin, server_log)
    kill: Kill | None = None
    try:
        vault = str(call(origin, token, "POST", "/api/v1/vaults", json.dumps({"name": f"long-{int(time.time())}"}).encode())["id"])
        load_at_start = load_average()
        begun = time.monotonic()
        uploaded = call(origin, token, "POST", f"/api/v1/vaults/{vault}/sources?name=long.pdf", args.pdf.read_bytes())
        source_row = uploaded["source"]
        assert isinstance(source_row, dict)
        source = str(source_row["id"])
        log.info("uploaded %s", kv(vault=vault, source=source, load=load_at_start))
        if args.mode == "kill":
            job_id: str | None = None
            first: int | None = None
            renewals = 0
            until = 0
            while renewals < 1:
                if time.monotonic() > deadline:
                    raise ScriptFailure("no renewal in time")
                job = wait_for_job(origin, token, vault, source, deadline)
                job_id = str(job["id"])
                if job["state"] in ("done", "dead", "cancelled"):
                    raise ScriptFailure(f"the job ended before a renewal {kv(state=str(job['state']))}")
                now = lease_until(args.container, args.database_url.rsplit("/", 1)[1], vault, job_id)
                if now is not None:
                    if first is None:
                        first = now
                    elif now != until and until != 0:
                        renewals += 1
                    until = now
                time.sleep(0.5)
            os.kill(server.pid, signal.SIGKILL)
            killed_at = int(time.time() * 1000)
            server.wait()
            assert job_id is not None
            left = lease_until(args.container, args.database_url.rsplit("/", 1)[1], vault, job_id)
            log.info("killed the server %s", kv(renewals=renewals, lease_left_ms=(left or 0) - killed_at))
            server, ready_ms = start_server(args.bin, env, origin, server_log)
            kill = Kill(killed_at_ms=killed_at, renewals_seen=renewals, lease_left_ms=(left or 0) - killed_at, new_server_ready_ms=ready_ms)
        ended = wait_for_end(origin, token, vault, source, deadline)
        total = time.monotonic() - begun
        attempts = ended["attempts"]
        assert isinstance(attempts, list)
        pages: int | None = None
        if ended["state"] == "done":
            count = call(origin, token, "GET", f"/api/v1/vaults/{vault}/sources/{source}/pages")["page_count"]
            assert isinstance(count, int)
            pages = count
        second = None
        if kill is not None and len(attempts) > 1:
            second = int(attempts[1]["started_at_ms"]) - kill.killed_at_ms
        return Report(
            state=str(ended["state"]),
            attempts=attempts,
            attempt_seconds=attempt_seconds(attempts),
            pages=pages,
            total_seconds=total,
            load_at_start=load_at_start,
            load_at_end=load_average(),
            kill=kill,
            second_attempt_after_kill_ms=second,
        )
    finally:
        server.kill()
        server.wait()


def main() -> int:
    args = parse_args()
    setup_logging(args.verbose)
    try:
        report = run(args)
    except (ScriptFailure, subprocess.CalledProcessError, urllib.error.URLError) as e:
        log.critical("%s", e)
        return 1
    print(report.model_dump_json(indent=2))
    log.info("done %s", kv(state=report.state, attempts=len(report.attempts), total_s=report.total_seconds))
    return 0


if __name__ == "__main__":
    sys.exit(main())
