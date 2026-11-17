#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = [
#     "pydantic>=2.7",
# ]
# ///
"""Restart or stop Postgres under a running memexvault server, and measure how it recovers.

What it does:
  Migrates and bootstraps an empty database, starts `memexvault serve`, creates a vault and
  uploads a few small documents. With --pdf it also uploads that PDF and waits until its
  rendition job is running. Then it stops the Postgres container for --outage-s seconds (0
  means a plain restart), starts it again, and uploads a small document every 250 ms until one
  answers 201. With --pdf it then waits for the job to end. It counts the server's log lines
  written from the stop until the first 201, and checks every acknowledged upload is still
  listed. It only ever stops the server it started. Spec 089's pass rules are in that spec.

Output:
  stdout  One JSON report of what was measured.
  stderr  Log lines: "YYYY-MM-DD HH:MM LEVEL message key=value ...".
          Levels: DEBUG (only with -v), INFO, WARN, ERROR, FAIL.

Exit codes:
  0  the run finished and the report is printed, whatever it measured
  1  failure (a FAIL line on stderr says why)
  2  invalid command line (printed by argparse)

Side effects:
  Writes to the database and data folder given, so point them at ones of your own. Stops and
  starts --container, so never point it at a shared one.

Examples:
  ./pg_restart.py --bin target/release/memexvault --database-url postgres://postgres:memex@127.0.0.1:55489/memex_s089 \\
      --data-dir target/s089/data --models-dir ~/.cache/memexvault-models --port 18189 --container memex-pg-s089 --cli podman
  ./pg_restart.py ... --outage-s 60
  ./pg_restart.py ... --pdf target/s089/long.pdf

--help output:
[BEGIN HELP]
  $ pg_restart.py --help
  usage: pg_restart.py [-h] [-v] --bin BIN --database-url URL --data-dir DIR
                       --models-dir DIR --port PORT --container NAME
                       [--cli CLI] [--outage-s S] [--pdf PDF] [--limit-s S]

  Restart or stop Postgres under a running memexvault server, and measure how it recovers.

  options:
    -h, --help         show this help message and exit
    -v, --verbose      log DEBUG lines too (default: False)
    --bin BIN          the memexvault binary (default: None)
    --database-url URL an empty database of your own (default: None)
    --data-dir DIR     a data folder of your own (default: None)
    --models-dir DIR   the installed models (default: None)
    --port PORT        the port to serve on (default: None)
    --container NAME   the Postgres container, which is stopped (default: None)
    --cli CLI          the container tool (default: docker)
    --outage-s S       how long Postgres stays stopped; 0 restarts it (default: 0)
    --pdf PDF          a long PDF whose rendition runs across the outage (default: None)
    --limit-s S        give up after this long (default: 1800)
[END HELP]
"""

from __future__ import annotations

import argparse
import json
import logging
import os
import re
import subprocess
import sys
import time
import urllib.error
import urllib.request
from pathlib import Path

from pydantic import BaseModel, ConfigDict

log = logging.getLogger("pg_restart")


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


class Report(BaseModel):
    model_config = ConfigDict(frozen=True)

    outage_s: int
    load_at_start: str
    load_at_end: str
    postgres_back_ms: int
    first_201_after_back_ms: int
    refused_after_back: int
    statuses_after_back: dict[str, int]
    log_lines_during_outage: int
    log_lines_by_message: dict[str, int]
    acknowledged: int
    missing: int
    job_state: str | None
    job_attempts: list[dict[str, object]]
    job_seconds_after_back: float | None


# --------------------------------------------------------------------------- pure core


_ANSI = re.compile(r"\x1b\[[0-9;]*m")


def message_of(line: str) -> str:
    """The message part of a tracing line, without its time, level, target and fields."""
    plain = _ANSI.sub("", line).strip()
    parts = plain.split(": ", 1)
    rest = parts[1] if len(parts) == 2 else plain
    return re.split(r"\s+\w+=", rest, maxsplit=1)[0].strip()


def count_messages(lines: list[str]) -> dict[str, int]:
    counts: dict[str, int] = {}
    for line in lines:
        if line.strip():
            m = message_of(line)
            counts[m] = counts.get(m, 0) + 1
    return dict(sorted(counts.items(), key=lambda kv_: -kv_[1]))


# --------------------------------------------------------------------------- io edge


def load_average() -> str:
    return repr(os.getloadavg())


def call(origin: str, token: str, method: str, path: str, body: bytes | None = None) -> tuple[int, dict[str, object]]:
    request = urllib.request.Request(f"{origin}{path}", data=body, method=method, headers={"authorization": f"Bearer {token}"})
    if body is not None and not path.split("?")[0].endswith("/sources"):
        request.add_header("content-type", "application/json")
    try:
        with urllib.request.urlopen(request, timeout=300) as response:
            return response.status, json.loads(response.read())
    except urllib.error.HTTPError as e:
        return e.code, {}


def must(result: tuple[int, dict[str, object]]) -> dict[str, object]:
    status, body = result
    if status >= 300:
        raise ScriptFailure(f"the server answered {status}")
    return body


def start_server(binary: Path, env: dict[str, str], origin: str, log_to: Path) -> subprocess.Popen[bytes]:
    started = time.monotonic()
    server = subprocess.Popen([str(binary), "serve"], env=env, stdout=log_to.open("ab"), stderr=subprocess.STDOUT)
    while time.monotonic() - started < 300:
        if server.poll() is not None:
            raise ScriptFailure(f"the server exited {kv(code=server.returncode, log=log_to)}")
        try:
            urllib.request.urlopen(f"{origin}/api/v1/health", timeout=5)
            return server
        except (urllib.error.URLError, OSError):
            time.sleep(0.1)
    server.kill()
    raise ScriptFailure("the server didn't start in 300 s")


def postgres_ready(cli: str, container: str) -> bool:
    return subprocess.run([cli, "exec", container, "pg_isready", "-U", "postgres"], capture_output=True).returncode == 0


def upload_small(origin: str, token: str, vault: str, n: int) -> tuple[int, str | None]:
    status, body = call(origin, token, "POST", f"/api/v1/vaults/{vault}/sources?name=d{n}.txt", f"restart document {n} {time.time()}".encode())
    source = body.get("source") if status == 201 else None
    return status, str(source["id"]) if isinstance(source, dict) else None


def job_of(origin: str, token: str, vault: str, source: str) -> dict[str, object] | None:
    status, body = call(origin, token, "GET", f"/api/v1/vaults/{vault}/jobs?kind=rendition&limit=200")
    if status != 200:
        return None
    items = body["items"]
    assert isinstance(items, list)
    return next((j for j in items if j["input"].get("source_id") == source), None)


def listed_sources(origin: str, token: str, vault: str) -> set[str]:
    seen: set[str] = set()
    after = ""
    while True:
        body = must(call(origin, token, "GET", f"/api/v1/vaults/{vault}/sources?limit=200{after}"))
        items = body["items"]
        assert isinstance(items, list)
        seen.update(str(i["id"]) for i in items)
        cursor = body.get("next_cursor")
        if not items or not cursor:
            return seen
        after = f"&cursor={cursor}"


# --------------------------------------------------------------------------- cli


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=(__doc__ or "").splitlines()[0], formatter_class=argparse.ArgumentDefaultsHelpFormatter)
    parser.add_argument("-v", "--verbose", action="store_true", help="log DEBUG lines too")
    parser.add_argument("--bin", type=Path, required=True, help="the memexvault binary")
    parser.add_argument("--database-url", required=True, metavar="URL", help="an empty database of your own")
    parser.add_argument("--data-dir", type=Path, required=True, metavar="DIR", help="a data folder of your own")
    parser.add_argument("--models-dir", type=Path, required=True, metavar="DIR", help="the installed models")
    parser.add_argument("--port", type=int, required=True, help="the port to serve on")
    parser.add_argument("--container", required=True, metavar="NAME", help="the Postgres container, which is stopped")
    parser.add_argument("--cli", default="docker", help="the container tool")
    parser.add_argument("--outage-s", type=int, default=0, metavar="S", help="how long Postgres stays stopped; 0 restarts it")
    parser.add_argument("--pdf", type=Path, help="a long PDF whose rendition runs across the outage")
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
    made = subprocess.run([str(args.bin), "bootstrap", "pg-restart"], env=env, check=True, capture_output=True, text=True).stdout
    token = made.strip().split()[-1]
    server = start_server(args.bin, env, origin, server_log)
    try:
        vault = str(must(call(origin, token, "POST", "/api/v1/vaults", json.dumps({"name": f"restart-{int(time.time())}"}).encode()))["id"])
        acked: set[str] = set()
        for n in range(5):
            status, source = upload_small(origin, token, vault, n)
            if source is None:
                raise ScriptFailure(f"an upload before the outage answered {status}")
            acked.add(source)
        pdf_source: str | None = None
        if args.pdf is not None:
            uploaded = must(call(origin, token, "POST", f"/api/v1/vaults/{vault}/sources?name=long.pdf", args.pdf.read_bytes()))
            row = uploaded["source"]
            assert isinstance(row, dict)
            pdf_source = str(row["id"])
            acked.add(pdf_source)
            while (job := job_of(origin, token, vault, pdf_source)) is None or job["state"] != "running":
                if time.monotonic() > deadline:
                    raise ScriptFailure("the rendition never started")
                time.sleep(0.2)
            time.sleep(5)
            log.info("the rendition is running %s", kv(job=str(job["id"])))
        load_at_start = load_average()
        lines_before = len(server_log.read_text(errors="replace").splitlines())
        if args.outage_s > 0:
            subprocess.run([args.cli, "stop", "-t", "10", args.container], check=True, capture_output=True)
            log.info("stopped postgres %s", kv(for_s=args.outage_s))
            end = time.monotonic() + args.outage_s
            n = 1000
            while time.monotonic() < end:
                n += 1
                status, _ = upload_small(origin, token, vault, n)
                if status == 201:
                    raise ScriptFailure("an upload was acknowledged while postgres was stopped")
                time.sleep(0.25)
            subprocess.run([args.cli, "start", args.container], check=True, capture_output=True)
        else:
            subprocess.run([args.cli, "restart", args.container], check=True, capture_output=True)
        started = time.monotonic()
        while not postgres_ready(args.cli, args.container):
            if time.monotonic() - started > 120:
                raise ScriptFailure("postgres didn't come back in 120 s")
            time.sleep(0.05)
        back = time.monotonic()
        postgres_back_ms = int((back - started) * 1000)
        log.info("postgres is back %s", kv(after_ms=postgres_back_ms))
        statuses: dict[str, int] = {}
        n = 10_000
        while True:
            n += 1
            status, source = upload_small(origin, token, vault, n)
            statuses[str(status)] = statuses.get(str(status), 0) + 1
            if source is not None:
                acked.add(source)
                break
            if time.monotonic() > deadline:
                raise ScriptFailure("no upload was acknowledged after postgres came back")
            time.sleep(0.25)
        first_201 = int((time.monotonic() - back) * 1000)
        log.info("an upload was acknowledged %s", kv(after_ms=first_201, statuses=json.dumps(statuses)))
        outage_lines = server_log.read_text(errors="replace").splitlines()[lines_before:]
        job_state, attempts, job_seconds = None, [], None
        if pdf_source is not None:
            while True:
                job = job_of(origin, token, vault, pdf_source)
                if job is not None and job["state"] in ("done", "dead", "cancelled"):
                    job_state, attempts = str(job["state"]), list(job["attempts"])  # type: ignore[arg-type]
                    job_seconds = time.monotonic() - back
                    break
                if time.monotonic() > deadline:
                    raise ScriptFailure("the rendition didn't end in time")
                time.sleep(1)
        missing = acked - listed_sources(origin, token, vault)
        return Report(
            outage_s=args.outage_s,
            load_at_start=load_at_start,
            load_at_end=load_average(),
            postgres_back_ms=postgres_back_ms,
            first_201_after_back_ms=first_201,
            refused_after_back=sum(v for k, v in statuses.items() if k != "201"),
            statuses_after_back=statuses,
            log_lines_during_outage=len([line for line in outage_lines if line.strip()]),
            log_lines_by_message=count_messages(outage_lines),
            acknowledged=len(acked),
            missing=len(missing),
            job_state=job_state,
            job_attempts=attempts,
            job_seconds_after_back=job_seconds,
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
    log.info("done %s", kv(first_201_ms=report.first_201_after_back_ms, log_lines=report.log_lines_during_outage, missing=report.missing))
    return 0


if __name__ == "__main__":
    sys.exit(main())
