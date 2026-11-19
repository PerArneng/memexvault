#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = [
#     "pydantic>=2.7",
#     "psycopg[binary]>=3.2",
# ]
# ///
"""Run two memexvault servers on one Postgres, each making a new vault every round, and count their connections.

What it does:
  Spec 093's item 4, the run that failed in spec 085's item 6. For each of two databases of your
  own on the same Postgres server it runs `migrate` and `bootstrap`, then starts `memexvault serve`
  with a pool limit of --pool and a writer limit of --writers. Each round, each server creates a
  new vault, then uploads one small markdown document to every vault it has and runs searches on
  them, from --threads threads. After --rounds it keeps uploading to every vault and searching,
  with no new vaults, until --minutes have passed since the start. A sampler reads `pg_stat_activity` every 50 ms: each server's
  connections by application name, how many of them hold an advisory lock, and the whole database
  server's count. It only ever stops the servers it started. Run the Postgres suites beside it
  for the full check.

Output:
  stdout  One JSON report of what was measured.
  stderr  Log lines: "YYYY-MM-DD HH:MM LEVEL message key=value ...".
          Levels: DEBUG (only with -v), INFO, WARN, ERROR, FAIL.

Exit codes:
  0  the run finished and the report is printed, whatever it measured
  1  failure (a FAIL line on stderr says why)
  2  invalid command line (printed by argparse)

Side effects:
  Writes to the two databases and data folders given, so point them at ones of your own. Starts
  two servers on --port and --port + 1, and stops them at the end.

Examples:
  ./two_servers.py --bin target/release/memexvault --admin-url postgres://postgres:memex@127.0.0.1:55493/memex \\
      --database s93a --database s93b --data-dir target/s093 --models-dir ~/.cache/memexvault-models --port 18193
  ./two_servers.py ... --rounds 24 --writers 8 --pool 10

--help output:
[BEGIN HELP]
  $ two_servers.py --help
  usage: two_servers.py [-h] [-v] --bin BIN --admin-url URL --database NAME
                        --data-dir DIR --models-dir DIR --port PORT
                        [--rounds N] [--minutes M] [--pool N] [--writers N]
                        [--threads N]

  Run two memexvault servers on one Postgres, each making a new vault every round, and count their connections.

  options:
    -h, --help        show this help message and exit
    -v, --verbose     log DEBUG lines too (default: False)
    --bin BIN         the memexvault binary (default: None)
    --admin-url URL   a URL to any database on the Postgres server, for the sampler and to create the two databases (default: None)
    --database NAME   a database of your own for one server; give it twice (default: None)
    --data-dir DIR    a folder of your own; each server gets a subfolder (default: None)
    --models-dir DIR  the installed models (default: None)
    --port PORT       the first server's port; the second takes the next one (default: None)
    --rounds N        how many vaults each server makes (default: 22)
    --minutes M       keep writing to every vault until this long after the start (default: 0)
    --pool N          MEMEX_PG_MAX_CONNECTIONS for each server (default: 10)
    --writers N       MEMEX_PG_WRITER_CONNECTIONS for each server (default: 8)
    --threads N       client threads per server (default: 8)
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
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
from collections import Counter
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

import psycopg
from pydantic import BaseModel, ConfigDict

log = logging.getLogger("two_servers")


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
        return json.dumps(value) if needs_quotes else value
    return str(value)


class ScriptFailure(Exception):
    """A failure the script reports with one FAIL line."""


# --------------------------------------------------------------------------- models


class ServerReport(BaseModel):
    model_config = ConfigDict(frozen=True)

    name: str
    vaults: int
    peak_connections: int
    peak_writers: int
    uploads: dict[str, int]
    searches: dict[str, int]
    too_many_clients_in_log: int


class Report(BaseModel):
    model_config = ConfigDict(frozen=True)

    max_connections: int
    pool: int
    writers: int
    rounds: int
    extra_rounds: int
    peak_database_server: int
    samples: int
    seconds: float
    load_before: str
    load_after: str
    servers: list[ServerReport]


# --------------------------------------------------------------------------- pure helpers


def with_app(url: str, database: str, app: str) -> str:
    """The admin URL pointed at another database, with an application name."""
    parts = urllib.parse.urlsplit(url)
    query = urllib.parse.urlencode({"application_name": app})
    return urllib.parse.urlunsplit((parts.scheme, parts.netloc, f"/{database}", query, ""))


def status_counts(statuses: list[int]) -> dict[str, int]:
    return {str(k): v for k, v in sorted(Counter(statuses).items())}


# --------------------------------------------------------------------------- IO


def call(origin: str, token: str, method: str, path: str, body: bytes | None = None, json_body: bool = False) -> tuple[int, dict[str, object]]:
    request = urllib.request.Request(f"{origin}{path}", data=body, method=method, headers={"authorization": f"Bearer {token}"})
    if json_body:
        request.add_header("content-type", "application/json")
    try:
        with urllib.request.urlopen(request, timeout=300) as response:
            return response.status, json.loads(response.read() or b"{}")
    except urllib.error.HTTPError as e:
        return e.code, {}
    except (urllib.error.URLError, OSError):
        return 0, {}


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


class Sampler(threading.Thread):
    """Reads `pg_stat_activity` every 50 ms: each app's connections and lock holders, and the server's total."""

    def __init__(self, admin_url: str, apps: list[str]) -> None:
        super().__init__(daemon=True)
        self.admin_url = admin_url
        self.apps = apps
        self.stop = threading.Event()
        self.peak: dict[str, int] = dict.fromkeys(apps, 0)
        self.peak_locks: dict[str, int] = dict.fromkeys(apps, 0)
        self.peak_total = 0
        self.samples = 0

    def run(self) -> None:
        with psycopg.connect(self.admin_url, autocommit=True) as conn:
            while not self.stop.is_set():
                rows = conn.execute(
                    "SELECT application_name, count(*), count(*) FILTER (WHERE EXISTS (SELECT 1 FROM pg_locks l WHERE l.pid = a.pid AND l.locktype = 'advisory')) "
                    "FROM pg_stat_activity a WHERE backend_type = 'client backend' GROUP BY 1"
                ).fetchall()
                self.peak_total = max(self.peak_total, sum(r[1] for r in rows))
                for app, count, locks in rows:
                    if app in self.peak:
                        self.peak[app] = max(self.peak[app], count)
                        self.peak_locks[app] = max(self.peak_locks[app], locks)
                self.samples += 1
                time.sleep(0.05)


class Load:
    """One server's client: a new vault each round, then an upload to every vault and searches, from several threads."""

    def __init__(self, origin: str, token: str, threads: int) -> None:
        self.origin = origin
        self.token = token
        self.threads = threads
        self.vaults: list[str] = []
        self.uploads: list[int] = []
        self.searches: list[int] = []
        self.n = 0

    def round(self, r: int, new_vault: bool = True) -> None:
        if new_vault:
            self.new_vault(r)
        jobs = []
        for v in self.vaults:
            self.n += 1
            jobs.append((v, self.n))
        with ThreadPoolExecutor(self.threads) as pool:
            self.uploads += pool.map(lambda j: self.upload(*j), jobs)
            self.searches += pool.map(self.search, [v for v in self.vaults for _ in range(3)])

    def new_vault(self, r: int) -> None:
        status, body = call(self.origin, self.token, "POST", "/api/v1/vaults", json.dumps({"name": f"r{r}-{time.time_ns()}"}).encode(), json_body=True)
        if status == 201 or status == 200:
            self.vaults.append(str(body["id"]))
        else:
            log.warning("can't create a vault %s", kv(origin=self.origin, status=status))

    def upload(self, vault: str, n: int) -> int:
        text = f"# Note {n}\n\nThe writer connection note number {n} about vault sharing and leases.\n"
        return call(self.origin, self.token, "POST", f"/api/v1/vaults/{vault}/sources?name=n{n}.md", text.encode())[0]

    def search(self, vault: str) -> int:
        return call(self.origin, self.token, "GET", f"/api/v1/vaults/{vault}/search/lexical?q=writer+connection")[0]


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    p = argparse.ArgumentParser(
        description="Run two memexvault servers on one Postgres, each making a new vault every round, and count their connections.",
        formatter_class=argparse.ArgumentDefaultsHelpFormatter,
    )
    p.add_argument("-v", "--verbose", action="store_true", help="log DEBUG lines too")
    p.add_argument("--bin", type=Path, required=True, help="the memexvault binary")
    p.add_argument("--admin-url", metavar="URL", required=True, help="a URL to any database on the Postgres server, for the sampler and to create the two databases")
    p.add_argument("--database", metavar="NAME", action="append", required=True, help="a database of your own for one server; give it twice")
    p.add_argument("--data-dir", metavar="DIR", type=Path, required=True, help="a folder of your own; each server gets a subfolder")
    p.add_argument("--models-dir", metavar="DIR", type=Path, required=True, help="the installed models")
    p.add_argument("--port", type=int, required=True, help="the first server's port; the second takes the next one")
    p.add_argument("--rounds", metavar="N", type=int, default=22, help="how many vaults each server makes")
    p.add_argument("--minutes", metavar="M", type=float, default=0, help="keep writing to every vault until this long after the start")
    p.add_argument("--pool", metavar="N", type=int, default=10, help="MEMEX_PG_MAX_CONNECTIONS for each server")
    p.add_argument("--writers", metavar="N", type=int, default=8, help="MEMEX_PG_WRITER_CONNECTIONS for each server")
    p.add_argument("--threads", metavar="N", type=int, default=8, help="client threads per server")
    args = p.parse_args(argv)
    if len(args.database) != 2:
        p.error("give --database twice")
    return args


def run(args: argparse.Namespace) -> Report:
    load_before = repr(os.getloadavg())
    with psycopg.connect(args.admin_url, autocommit=True) as admin:
        max_connections = int(admin.execute("SHOW max_connections").fetchone()[0])
        for db in args.database:
            admin.execute(f'DROP DATABASE IF EXISTS "{db}" WITH (FORCE)')
            admin.execute(f'CREATE DATABASE "{db}"')
    servers: list[subprocess.Popen[bytes]] = []
    loads: list[Load] = []
    logs: list[Path] = []
    apps = [f"s093_{db}" for db in args.database]
    sampler = Sampler(args.admin_url, apps)
    started = time.monotonic()
    extra = 0
    try:
        for i, (db, app) in enumerate(zip(args.database, apps, strict=True)):
            data = args.data_dir / db
            data.mkdir(parents=True, exist_ok=True)
            env = dict(os.environ)
            env.update(
                MEMEX_DATABASE_URL=with_app(args.admin_url, db, app),
                MEMEX_DATA_DIR=str(data),
                MEMEX_MODELS_DIR=str(args.models_dir),
                MEMEX_HTTP_BIND=f"127.0.0.1:{args.port + i}",
                MEMEX_PG_MAX_CONNECTIONS=str(args.pool),
                MEMEX_PG_WRITER_CONNECTIONS=str(args.writers),
            )
            subprocess.run([str(args.bin), "migrate"], env=env, check=True, capture_output=True)
            made = subprocess.run([str(args.bin), "bootstrap", "load"], env=env, check=True, capture_output=True, text=True).stdout
            token = made.strip().split()[-1]
            origin = f"http://127.0.0.1:{args.port + i}"
            log_to = data / "server.log"
            logs.append(log_to)
            servers.append(start_server(args.bin, env, origin, log_to))
            loads.append(Load(origin, token, args.threads))
            log.info("server started %s", kv(database=db, origin=origin))
        sampler.start()
        for r in range(args.rounds):
            with ThreadPoolExecutor(2) as pool:
                list(pool.map(lambda load: load.round(r), loads))
            log.info(
                "round done %s",
                kv(round=r + 1, vaults=[len(x.vaults) for x in loads], peak=sampler.peak, writers=sampler.peak_locks, total=sampler.peak_total),
            )
        while time.monotonic() - started < args.minutes * 60:
            with ThreadPoolExecutor(2) as pool:
                list(pool.map(lambda load: load.round(0, new_vault=False), loads))
            extra += 1
            if extra % 20 == 0:
                log.info("rounds with no new vault %s", kv(done=extra, peak=sampler.peak, writers=sampler.peak_locks, total=sampler.peak_total))
    finally:
        sampler.stop.set()
        for s in servers:
            s.terminate()
        for s in servers:
            try:
                s.wait(timeout=30)
            except subprocess.TimeoutExpired:
                s.kill()
    sampler.join(timeout=5)
    reports = []
    for db, app, load, log_to in zip(args.database, apps, loads, logs, strict=True):
        text = log_to.read_text(errors="replace")
        reports.append(
            ServerReport(
                name=db,
                vaults=len(load.vaults),
                peak_connections=sampler.peak[app],
                peak_writers=sampler.peak_locks[app],
                uploads=status_counts(load.uploads),
                searches=status_counts(load.searches),
                too_many_clients_in_log=text.count("too many clients"),
            )
        )
    return Report(
        max_connections=max_connections,
        pool=args.pool,
        writers=args.writers,
        rounds=args.rounds,
        extra_rounds=extra,
        peak_database_server=sampler.peak_total,
        samples=sampler.samples,
        seconds=round(time.monotonic() - started, 1),
        load_before=load_before,
        load_after=repr(os.getloadavg()),
        servers=reports,
    )


def main(argv: list[str] | None = None) -> int:
    args = parse_args(argv)
    setup_logging(args.verbose)
    try:
        report = run(args)
    except (ScriptFailure, subprocess.CalledProcessError, psycopg.Error) as e:
        log.critical("%s", e)
        return 1
    print(report.model_dump_json(indent=2))
    return 0


if __name__ == "__main__":
    sys.exit(main())
