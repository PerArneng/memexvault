#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = [
#     "pydantic>=2.7",
# ]
# ///
"""Run files through a real memexvault server and report how each one's job ended.

What it does:
  Migrates and bootstraps an empty database, starts `memexvault serve` with any extra
  settings given by --env, uploads every --file to one new vault at once, and waits until
  the rendition or transcription job of each has ended. Spec 084's item 3 runs it with the
  default ceilings on a 400-page PDF and a 60-minute recording, which must each end `done`
  in 1 attempt. With a short ceiling, such as --env MEMEX_RENDITION_CEILING_S=20, it shows
  the ceiling on a real server: each attempt fails with the ceiling's reason. It only ever
  stops the server it started.

Output:
  stdout  One JSON report of what was measured.
  stderr  Log lines: "YYYY-MM-DD HH:MM LEVEL message key=value ...".
          Levels: DEBUG (only with -v), INFO, WARN, ERROR, FAIL.

Exit codes:
  0  the run finished and the report is printed, whatever the jobs' outcomes
  1  failure (a FAIL line on stderr says why)
  2  invalid command line (printed by argparse)

Side effects:
  Writes to the database and data folder given, so point them at ones of your own.

Examples:
  ./ceiling_run.py --bin target/release/memexvault --database-url postgres://postgres:memex@127.0.0.1:55432/memex_s084 \\
      --data-dir target/s084/data --models-dir ~/.cache/memexvault-models --port 18184 \\
      --file target/s084/long.pdf --file target/s084/speech_60m.mp3

--help output:
[BEGIN HELP]
  $ ceiling_run.py --help
  usage: ceiling_run.py [-h] [-v] --bin BIN --database-url URL --data-dir DIR
                        --models-dir DIR --port PORT --file FILE
                        [--env KEY=VALUE] [--limit-s S]

  Run files through a real memexvault server and report how each one's job
  ended.

  options:
    -h, --help          show this help message and exit
    -v, --verbose       log DEBUG lines too (default: False)
    --bin BIN           the memexvault binary (default: None)
    --database-url URL  an empty database of your own (default: None)
    --data-dir DIR      a data folder of your own (default: None)
    --models-dir DIR    the installed models (default: None)
    --port PORT         the port to serve on (default: None)
    --file FILE         a file to upload; give it once per file (default: None)
    --env KEY=VALUE     a setting for the server; give it once per setting
                        (default: [])
    --limit-s S         give up after this long (default: 3600)
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
import urllib.parse
import urllib.request
from pathlib import Path

from pydantic import BaseModel, ConfigDict

log = logging.getLogger("ceiling_run")


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


class FileReport(BaseModel):
    model_config = ConfigDict(frozen=True)

    file: str
    kind: str
    state: str
    attempts: int
    attempt_seconds: list[float]
    attempt_errors: list[str | None]
    seconds_from_upload: float
    load_at_end: list[float]


class Report(BaseModel):
    model_config = ConfigDict(frozen=True)

    settings: dict[str, str]
    load_at_start: list[float]
    load_at_end: list[float]
    server_peak_rss_mb: float | None
    server_threads_at_end: int | None
    files: list[FileReport]


# --------------------------------------------------------------------------- pure core


def attempt_seconds(attempts: list[dict[str, object]]) -> list[float]:
    spans = []
    for a in attempts:
        start, end = a.get("started_at_ms"), a.get("finished_at_ms")
        if isinstance(start, int) and isinstance(end, int):
            spans.append((end - start) / 1000)
    return spans


def parse_env(pairs: list[str]) -> dict[str, str]:
    out = {}
    for pair in pairs:
        key, sep, value = pair.partition("=")
        if not sep or not key:
            raise ScriptFailure(f"--env needs KEY=VALUE {kv(got=pair)}")
        out[key] = value
    return out


def proc_status(text: str, field: str) -> str | None:
    for line in text.splitlines():
        if line.startswith(f"{field}:"):
            return line.split(":", 1)[1].strip()
    return None


# --------------------------------------------------------------------------- io edge


def load_average() -> list[float]:
    return [round(x, 1) for x in os.getloadavg()]


def call(origin: str, token: str, method: str, path: str, body: bytes | None = None) -> dict[str, object]:
    request = urllib.request.Request(f"{origin}{path}", data=body, method=method, headers={"authorization": f"Bearer {token}"})
    if body is not None and not path.split("?")[0].endswith("/sources"):
        request.add_header("content-type", "application/json")
    with urllib.request.urlopen(request, timeout=300) as response:
        return json.loads(response.read())


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


def find_job(origin: str, token: str, vault: str, source: str) -> dict[str, object] | None:
    for kind in ("rendition", "transcription"):
        items = call(origin, token, "GET", f"/api/v1/vaults/{vault}/jobs?kind={kind}&limit=200")["items"]
        assert isinstance(items, list)
        for job in items:
            if job["input"].get("source_id") == source:
                return job
    return None


# --------------------------------------------------------------------------- cli


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=(__doc__ or "").splitlines()[0], formatter_class=argparse.ArgumentDefaultsHelpFormatter)
    parser.add_argument("-v", "--verbose", action="store_true", help="log DEBUG lines too")
    parser.add_argument("--bin", type=Path, required=True, help="the memexvault binary")
    parser.add_argument("--database-url", required=True, metavar="URL", help="an empty database of your own")
    parser.add_argument("--data-dir", type=Path, required=True, metavar="DIR", help="a data folder of your own")
    parser.add_argument("--models-dir", type=Path, required=True, metavar="DIR", help="the installed models")
    parser.add_argument("--port", type=int, required=True, help="the port to serve on")
    parser.add_argument("--file", type=Path, action="append", required=True, help="a file to upload; give it once per file")
    parser.add_argument("--env", action="append", default=[], metavar="KEY=VALUE", help="a setting for the server; give it once per setting")
    parser.add_argument("--limit-s", type=int, default=3600, metavar="S", help="give up after this long")
    return parser.parse_args(argv)


def run(args: argparse.Namespace) -> Report:
    extra = parse_env(args.env)
    env = {
        **os.environ,
        "MEMEX_DATABASE_URL": args.database_url,
        "MEMEX_DATA_DIR": str(args.data_dir),
        "MEMEX_MODELS_DIR": str(args.models_dir),
        "MEMEX_HTTP_BIND": f"127.0.0.1:{args.port}",
        **extra,
    }
    origin = f"http://127.0.0.1:{args.port}"
    deadline = time.monotonic() + args.limit_s
    args.data_dir.mkdir(parents=True, exist_ok=True)
    server_log = args.data_dir / "server.log"
    subprocess.run([str(args.bin), "migrate"], env=env, check=True, capture_output=True)
    made = subprocess.run([str(args.bin), "bootstrap", f"ceiling-{int(time.time())}"], env=env, check=True, capture_output=True, text=True).stdout
    token = made.strip().split()[-1]
    server = start_server(args.bin, env, origin, server_log)
    try:
        vault = str(call(origin, token, "POST", "/api/v1/vaults", json.dumps({"name": f"ceiling-{int(time.time())}"}).encode())["id"])
        load_at_start = load_average()
        sources: dict[str, tuple[Path, float]] = {}
        for path in args.file:
            name = urllib.parse.quote(path.name)
            uploaded = call(origin, token, "POST", f"/api/v1/vaults/{vault}/sources?name={name}", path.read_bytes())
            row = uploaded["source"]
            assert isinstance(row, dict)
            sources[str(row["id"])] = (path, time.monotonic())
            log.info("uploaded %s", kv(file=path, source=row["id"], load=str(load_at_start)))
        reports: dict[str, FileReport] = {}
        peak: float | None = None
        threads: int | None = None
        while len(reports) < len(sources):
            if time.monotonic() > deadline:
                raise ScriptFailure(f"the jobs didn't end in time {kv(ended=len(reports), of=len(sources))}")
            try:
                status = Path(f"/proc/{server.pid}/status").read_text()
                hwm = proc_status(status, "VmHWM")
                peak = int(hwm.split()[0]) / 1024 if hwm else peak
                found = proc_status(status, "Threads")
                threads = int(found) if found else threads
            except OSError:
                pass
            for source, (path, began) in sources.items():
                if source in reports:
                    continue
                try:
                    job = find_job(origin, token, vault, source)
                except (urllib.error.URLError, OSError) as e:
                    log.warning("can't read the job yet %s", kv(detail=str(e)))
                    continue
                if job is None or job["state"] not in ("done", "dead", "cancelled"):
                    continue
                attempts = job["attempts"]
                assert isinstance(attempts, list)
                reports[source] = FileReport(
                    file=path.name,
                    kind=str(job["kind"]),
                    state=str(job["state"]),
                    attempts=len(attempts),
                    attempt_seconds=attempt_seconds(attempts),
                    attempt_errors=[a.get("error") for a in attempts],
                    seconds_from_upload=round(time.monotonic() - began, 1),
                    load_at_end=load_average(),
                )
                log.info("ended %s", kv(file=path.name, state=str(job["state"]), attempts=len(attempts)))
            time.sleep(1)
        return Report(
            settings=extra,
            load_at_start=load_at_start,
            load_at_end=load_average(),
            server_peak_rss_mb=round(peak, 1) if peak is not None else None,
            server_threads_at_end=threads,
            files=list(reports.values()),
        )
    finally:
        server.kill()
        server.wait()


def main() -> int:
    args = parse_args()
    setup_logging(args.verbose)
    try:
        report = run(args)
    except ScriptFailure as e:
        log.critical("%s", e)
        return 1
    print(report.model_dump_json(indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
