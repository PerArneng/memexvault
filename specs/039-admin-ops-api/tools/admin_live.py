#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = [
#     "pydantic>=2.7",
#     "psycopg[binary]>=3.2",
# ]
# ///
"""Check spec 039's admin routes on the real binary and real Postgres, with Postgres cut off behind a proxy.

What it does:
  Makes a role whose password is a marker and a database of its own, and starts a TCP proxy to
  Postgres that it can cut. It runs `memexvault migrate`, `bootstrap` and `serve` through that
  proxy, with the LLM key and the transcription key also set to the marker. The LLM and the
  transcription API point at 127.0.0.1:1, so nothing is ever sent to a provider. It uploads one
  document, then runs every operation through POST /vaults/{id}/operations, polling the job for
  its progress, and compares the results with the commands: `reconcile --at` against the reconcile
  report, and `restore` plus `reconcile` against the export. It imports the export over HTTP,
  without the replace flag (which must be refused) and with a new id. It times GET
  /api/v1/system/status with everything up, with Postgres cut (connections refused), with Postgres
  stalled (connections open and nothing answering), and after Postgres is back each time. At the
  end it stops the server and looks for the marker in every response body and in the server's
  and the commands' output. It drops the database and the role it made.

Output:
  stdout  One JSON report of what was measured.
  stderr  Log lines: "YYYY-MM-DD HH:MM LEVEL message key=value ...".
          Levels: DEBUG (only with -v), INFO, WARN, ERROR, FAIL.

Exit codes:
  0  every check passed, and the report is printed
  1  a check failed, or the run couldn't finish (a FAIL line on stderr says why)
  2  invalid command line (printed by argparse)

Side effects:
  Creates and drops a role and a database on the Postgres server --admin-url names, and writes
  under --data-dir, which it empties first. Starts one server on --port and stops it at the end.

Examples:
  ./admin_live.py --bin target/debug/memexvault --admin-url postgres://postgres:memex@127.0.0.1:55432/memex \\
      --data-dir target/s039 --models-dir ~/.cache/memexvault-models --port 18039

--help output:
[BEGIN HELP]
  $ admin_live.py --help
  usage: admin_live.py [-h] [-v] --bin BIN --admin-url URL --data-dir DIR
                       --models-dir DIR --port PORT

  Check spec 039's admin routes on the real binary and real Postgres, with Postgres cut off behind a proxy.

  options:
    -h, --help        show this help message and exit
    -v, --verbose     log DEBUG lines too (default: False)
    --bin BIN         the memexvault binary (default: None)
    --admin-url URL   a superuser's URL to any database on the Postgres server (default: None)
    --data-dir DIR    a folder of your own, emptied first (default: None)
    --models-dir DIR  the installed models (default: None)
    --port PORT       the server's port; the proxy takes the next one (default: None)
[END HELP]
"""

from __future__ import annotations

import argparse
import json
import logging
import os
import re
import shutil
import signal
import socket
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

import psycopg
from pydantic import BaseModel, ConfigDict

log = logging.getLogger("admin_live")

MARKER = "ZQXS039SECRET7741"

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


class OperationRun(BaseModel):
    model_config = ConfigDict(frozen=True)

    op: str
    job: str
    state: str
    seconds: float
    progress_seen: list[str]
    report: dict[str, object] | None


class StatusTiming(BaseModel):
    model_config = ConfigDict(frozen=True)

    when: str
    http: int
    ms: float
    postgres_reachable: bool | None
    llm_reachable: bool | None
    caller_from_memory: bool | None
    vaults_error: str | None


class Report(BaseModel):
    model_config = ConfigDict(frozen=True)

    operations: list[OperationRun]
    second_reindex_http: int
    second_reindex_message: str
    reconcile_matches_command: bool
    export_restored_by_command_matches: bool
    import_without_replace_http: int
    import_without_replace_message: str
    import_new_id_matches: bool
    status: list[StatusTiming]
    seconds_to_recover: float
    responses_checked: int
    marker_in_responses: int
    marker_in_logs: int
    log_bytes: int


# --------------------------------------------------------------------------- pure helpers


def server_url(user: str, password: str, port: int, database: str) -> str:
    """A URL for the role, through the proxy on this machine."""
    return f"postgres://{user}:{password}@127.0.0.1:{port}/{database}"


def parse_reconcile(text: str) -> dict[str, str]:
    """The fields `memexvault reconcile` prints that a report has too."""
    out: dict[str, str] = {}
    for line in text.splitlines():
        parts = line.split()
        if parts[:1] == ["tables"] and len(parts) == 2:
            out["tables"] = parts[1]
        elif parts[:1] == ["at"] and len(parts) == 3:
            out["at_seq"], out["at_sha"] = parts[1], parts[2]
        elif parts[:2] == ["blobs", "referenced"]:
            out["blobs_referenced"], out["blobs_stored"] = parts[2], parts[4]
    return out


# --------------------------------------------------------------------------- edges


class Proxy:
    """Forwards a port to Postgres, and can be cut and restored: cutting closes every connection and refuses new ones."""

    def __init__(self, listen_port: int, target: tuple[str, int]) -> None:
        self.listen_port, self.target = listen_port, target
        self.lock = threading.Lock()
        self.sockets: list[socket.socket] = []
        self.listener: socket.socket | None = None
        # While set, connections stay open and every byte is dropped, as with a server that has stopped answering.
        self.stalled = threading.Event()

    def start(self) -> None:
        listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        listener.bind(("127.0.0.1", self.listen_port))
        listener.listen(64)
        self.listener = listener
        threading.Thread(target=self._accept, args=(listener,), daemon=True).start()

    def _accept(self, listener: socket.socket) -> None:
        while True:
            try:
                client, _ = listener.accept()
            except OSError:
                return
            if self.stalled.is_set():
                with self.lock:
                    self.sockets.append(client)
                continue
            try:
                upstream = socket.create_connection(self.target, timeout=5)
                upstream.settimeout(None)
            except OSError:
                client.close()
                continue
            with self.lock:
                self.sockets += [client, upstream]
            for a, b in ((client, upstream), (upstream, client)):
                threading.Thread(target=self._pipe, args=(a, b), daemon=True).start()

    def _pipe(self, a: socket.socket, b: socket.socket) -> None:
        try:
            while data := a.recv(65536):
                if not self.stalled.is_set():
                    b.sendall(data)
        except OSError:
            pass
        for s in (a, b):
            try:
                s.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass

    def cut(self) -> None:
        if self.listener:
            # Closing alone doesn't wake a thread blocked in accept() on Linux, which would go on accepting.
            try:
                self.listener.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass
            self.listener.close()
            self.listener = None
        with self.lock:
            for s in self.sockets:
                try:
                    s.shutdown(socket.SHUT_RDWR)
                    s.close()
                except OSError:
                    pass
            self.sockets = []


class Http:
    """Calls the server with the admin's token, keeping every body to look for the marker in."""

    def __init__(self, base: str, token: str) -> None:
        self.base, self.token = base, token
        self.bodies: list[bytes] = []

    def call(self, method: str, path: str, body: bytes | None = None, json_body: object = None, timeout: float = 30) -> tuple[int, bytes]:
        data = body if body is not None else (json.dumps(json_body).encode() if json_body is not None else None)
        req = urllib.request.Request(self.base + path, data=data, method=method)
        req.add_header("authorization", f"Bearer {self.token}")
        if json_body is not None:
            req.add_header("content-type", "application/json")
        try:
            with urllib.request.urlopen(req, timeout=timeout) as res:
                out = (res.status, res.read())
        except urllib.error.HTTPError as e:
            out = (e.code, e.read())
        self.bodies.append(out[1])
        return out

    def json(self, method: str, path: str, **kw: object) -> tuple[int, dict]:
        status, raw = self.call(method, path, **kw)  # type: ignore[arg-type]
        try:
            return status, json.loads(raw) if raw else {}
        except json.JSONDecodeError:
            return status, {"raw": raw.decode(errors="replace")}


def run_cmd(binary: str, args: list[str], env: dict[str, str], logs: Path) -> str:
    out = subprocess.run([binary, *args], env=env, capture_output=True, text=True, timeout=600)
    with logs.open("a") as f:
        f.write(f"$ memexvault {' '.join(args)}\n{out.stdout}{out.stderr}\n")
    if out.returncode not in (0, 1):
        raise ScriptFailure(f"memexvault {args[0]} failed: {out.stderr.strip()[-400:]}")
    return out.stdout


# --------------------------------------------------------------------------- the run


def wait_for(check, seconds: float, what: str) -> float:
    start = time.monotonic()
    while time.monotonic() - start < seconds:
        if check():
            return time.monotonic() - start
        time.sleep(0.05)
    raise ScriptFailure(f"timed out waiting for {what}")


def run_operation(http: Http, vault: str, body: dict) -> OperationRun:
    start = time.monotonic()
    status, job = http.json("POST", f"/api/v1/vaults/{vault}/operations", json_body=body)
    if status != 202:
        raise ScriptFailure(f"starting {body} answered {status}: {job}")
    seen: list[str] = []
    while True:
        _, job = http.json("GET", f"/api/v1/vaults/{vault}/jobs/{job['id']}")
        p = job.get("progress")
        tag = f"{job['state']}:{p['step']}/{p['of']} {p['label']}" if p else job["state"]
        if not seen or seen[-1] != tag:
            seen.append(tag)
        if job["state"] in ("done", "dead", "cancelled"):
            break
        if time.monotonic() - start > 300:
            raise ScriptFailure(f"{body} didn't finish: {job}")
        time.sleep(0.05)
    run = OperationRun(op=json.dumps(body), job=job["id"], state=job["state"], seconds=round(time.monotonic() - start, 3), progress_seen=seen, report=job.get("report"))
    log.info(kv(op=run.op, state=run.state, seconds=run.seconds, steps=len(seen)))
    if run.state != "done":
        raise ScriptFailure(f"{body} ended {run.state}: {job.get('attempts')}")
    return run


def status_timing(http: Http, when: str) -> StatusTiming:
    start = time.monotonic()
    code, body = http.json("GET", "/api/v1/system/status", timeout=10)
    ms = round((time.monotonic() - start) * 1000, 1)
    t = StatusTiming(
        when=when,
        http=code,
        ms=ms,
        postgres_reachable=body.get("postgres", {}).get("reachable"),
        llm_reachable=body.get("llm", {}).get("reachable"),
        caller_from_memory=body.get("caller_from_memory"),
        vaults_error=body.get("vaults_error"),
    )
    log.info(kv(when=when, http=code, ms=ms, postgres=t.postgres_reachable, llm=t.llm_reachable, from_memory=t.caller_from_memory))
    return t


def main_run(args: argparse.Namespace) -> Report:
    suffix = f"s039_{os.getpid()}"
    role, database = suffix, f"memex_{suffix}"
    admin = psycopg.connect(args.admin_url, autocommit=True)
    parts = urllib.parse.urlsplit(args.admin_url)
    data_dir = Path(args.data_dir)
    shutil.rmtree(data_dir, ignore_errors=True)
    data_dir.mkdir(parents=True)
    logs = data_dir / "server.log"
    proxy = Proxy(args.port + 1, (parts.hostname or "127.0.0.1", parts.port or 5432))
    server: subprocess.Popen | None = None
    admin.execute(f"CREATE ROLE {role} LOGIN SUPERUSER PASSWORD '{MARKER}'")
    try:
        admin.execute(f"CREATE DATABASE {database} OWNER {role}")
        proxy.start()
        env = {
            **os.environ,
            "MEMEX_DATABASE_URL": server_url(role, MARKER, args.port + 1, database),
            "MEMEX_DATA_DIR": str(data_dir / "data"),
            "MEMEX_MODELS_DIR": str(Path(args.models_dir).expanduser()),
            "MEMEX_HTTP_BIND": f"127.0.0.1:{args.port}",
            "MEMEX_PG_WAIT_MS": "1000",
            "MEMEX_LLM_PROVIDER": "openai",
            "MEMEX_LLM_MODEL": "nobody",
            "MEMEX_LLM_BASE_URL": "http://127.0.0.1:1/v1",
            "MEMEX_LLM_API_KEY": MARKER,
            "MEMEX_BACKEND_TRANSCRIBER": "gemini",
            "MEMEX_TRANSCRIBE_API_MODEL": "nobody",
            "MEMEX_TRANSCRIBE_API_KEY": MARKER,
            "MEMEX_TRANSCRIBE_API_BASE_URL": "http://127.0.0.1:1",
            "RUST_LOG": "info",
        }
        run_cmd(args.bin, ["migrate"], env, logs)
        token = run_cmd(args.bin, ["bootstrap", "root"], env, logs).strip().splitlines()[-1]
        with logs.open("a") as f:
            server = subprocess.Popen([args.bin, "serve"], env=env, stdout=f, stderr=subprocess.STDOUT)
        http = Http(f"http://127.0.0.1:{args.port}", token)

        def healthy() -> bool:
            try:
                return http.call("GET", "/api/v1/health", timeout=2)[0] == 200
            except OSError:
                return False

        wait_for(healthy, 120, "the server")
        _, vault = http.json("POST", "/api/v1/vaults", json_body={"name": "spec-39"})
        vid = vault["id"]
        code, up = http.call("POST", f"/api/v1/vaults/{vid}/sources?name=notes.txt", body=b"Ferries leave the harbour every hour. The harbour master keeps the timetable.")
        if code not in (200, 201):
            raise ScriptFailure(f"upload answered {code}: {up!r}")

        def renditions_done() -> bool:
            _, jobs = http.json("GET", f"/api/v1/vaults/{vid}/jobs?kind=rendition&limit=100")
            return bool(jobs.get("items")) and all(j["state"] == "done" for j in jobs["items"])

        wait_for(renditions_done, 120, "the document's rendition")
        ops: list[OperationRun] = []
        ops.append(run_operation(http, vid, {"op": "reindex", "index": "vector"}))
        # The second of the same kind, while the first is queued or running.
        code, first = http.json("POST", f"/api/v1/vaults/{vid}/operations", json_body={"op": "reindex", "index": "keyword"})
        if code != 202:
            raise ScriptFailure(f"reindex answered {code}: {first}")
        second_code, second = http.json("POST", f"/api/v1/vaults/{vid}/operations", json_body={"op": "reindex", "index": "keyword"})
        second_message = second.get("error", {}).get("message", "")
        log.info(kv(second_reindex=second_code, message=second_message))
        wait_for(lambda: http.json("GET", f"/api/v1/vaults/{vid}/jobs/{first['id']}")[1]["state"] == "done", 300, "the keyword reindex")
        ops.append(run_operation(http, vid, {"op": "rollback", "index": "keyword"}))
        ops.append(run_operation(http, vid, {"op": "reconcile", "deep": True}))
        ops.append(run_operation(http, vid, {"op": "sweep", "grace_s": 0}))
        ops.append(run_operation(http, vid, {"op": "compact"}))
        export = run_operation(http, vid, {"op": "export"})
        ops.append(export)

        reconcile = ops[2].report or {}
        cli = parse_reconcile(run_cmd(args.bin, ["reconcile", vid, "--at", str(reconcile["head"])], env, logs))
        reconcile_matches = cli.get("at_sha") == reconcile["tables_sha256"] and cli.get("blobs_stored") == str(reconcile["blobs_stored"])
        log.info(kv(reconcile_matches=reconcile_matches, report=reconcile["tables_sha256"], command=cli.get("at_sha")))

        code, archive = http.call("GET", f"/api/v1/vaults/{vid}/operations/{export.job}/archive")
        if code != 200:
            raise ScriptFailure(f"the archive answered {code}")
        archive_file = data_dir / "export.memex"
        archive_file.write_bytes(archive)
        dump = (export.report or {})["dump_sha256"]

        refused_code, refused = http.json("POST", "/api/v1/imports", body=archive)
        refused_message = refused.get("error", {}).get("message", "")
        log.info(kv(import_without_replace=refused_code, message=refused_message))
        code, started = http.json("POST", "/api/v1/imports?new_id=true&name=by-http", body=archive)
        if code != 202:
            raise ScriptFailure(f"the import answered {code}: {started}")
        imported: dict = {}

        def import_done() -> bool:
            nonlocal imported
            imported = http.json("GET", f"/api/v1/imports/{started['id']}")[1]
            return imported.get("state") != "running"

        wait_for(import_done, 300, "the import")
        if imported.get("state") != "done":
            raise ScriptFailure(f"the import ended {imported}")

        statuses = [status_timing(http, "up")]
        proxy.cut()
        log.info("Postgres cut off")
        for n in range(3):
            statuses.append(status_timing(http, f"postgres cut {n + 1}"))
            time.sleep(0.5)
        code, _ = http.json("GET", "/api/v1/system/settings")
        log.info(kv(settings_while_down=code))
        proxy.start()
        log.info("Postgres back")
        recover = wait_for(lambda: status_timing(http, "recovering").postgres_reachable is True, 60, "Postgres to be reachable again")
        statuses.append(status_timing(http, "up again"))
        proxy.stalled.set()
        log.info("Postgres stalled: connections stay open and nothing answers")
        for n in range(3):
            statuses.append(status_timing(http, f"postgres stalled {n + 1}"))
            time.sleep(0.5)
        proxy.cut()
        proxy.stalled.clear()
        proxy.start()
        log.info("Postgres back")
        recover = max(recover, wait_for(lambda: status_timing(http, "recovering").postgres_reachable is True, 60, "Postgres to be reachable again"))
        statuses.append(status_timing(http, "back"))
        server.send_signal(signal.SIGINT)
        server.wait(timeout=60)
        server = None
        # A serving server takes the lease of every new vault, so the command restores once it has stopped.
        by_http = parse_reconcile(run_cmd(args.bin, ["reconcile", imported["report"]["vault_id"]], env, logs))
        import_matches = by_http.get("tables") == dump
        restored = run_cmd(args.bin, ["restore", str(archive_file), "--new-id", "--name", "by-command"], env, logs)
        by_command = parse_reconcile(run_cmd(args.bin, ["reconcile", restored.split()[2]], env, logs))
        export_matches = by_command.get("tables") == dump
        log.info(kv(export_matches=export_matches, import_matches=import_matches, archive_bytes=len(archive)))
        log_text = logs.read_text(errors="replace")
        report = Report(
            operations=ops,
            second_reindex_http=second_code,
            second_reindex_message=second_message,
            reconcile_matches_command=reconcile_matches,
            export_restored_by_command_matches=export_matches,
            import_without_replace_http=refused_code,
            import_without_replace_message=refused_message,
            import_new_id_matches=import_matches,
            status=statuses,
            seconds_to_recover=round(recover, 2),
            responses_checked=len(http.bodies),
            marker_in_responses=sum(MARKER.encode() in b for b in http.bodies),
            marker_in_logs=log_text.count(MARKER),
            log_bytes=len(log_text),
        )
        return report
    finally:
        if server is not None:
            server.kill()
            server.wait()
        proxy.cut()
        admin.execute(f"DROP DATABASE IF EXISTS {database} WITH (FORCE)")
        admin.execute(f"DROP ROLE IF EXISTS {role}")
        admin.close()


def failures(r: Report) -> list[str]:
    out = []
    if r.second_reindex_http != 409 or "already" not in r.second_reindex_message:
        out.append(f"a second keyword reindex answered {r.second_reindex_http}")
    if not r.reconcile_matches_command:
        out.append("the reconcile report differs from the command")
    if not r.export_restored_by_command_matches or not r.import_new_id_matches:
        out.append("a restored export differs from its dump hash")
    if r.import_without_replace_http != 409 or "only when told to replace it" not in r.import_without_replace_message:
        out.append("an import over an existing vault wasn't refused")
    for s in r.status:
        if s.ms >= 2000:
            out.append(f"status took {s.ms} ms {s.when}")
    cut = [s for s in r.status if s.when.startswith("postgres ")]
    if not all(s.http == 200 and s.postgres_reachable is False and s.caller_from_memory for s in cut):
        out.append("status didn't report Postgres down")
    if r.status[0].postgres_reachable is not True or r.status[0].llm_reachable is not False:
        out.append("status was wrong with Postgres up and the LLM away")
    if r.marker_in_responses or r.marker_in_logs:
        out.append(f"the marker appeared: responses={r.marker_in_responses} logs={r.marker_in_logs}")
    return out


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(
        prog="admin_live.py",
        description="Check spec 039's admin routes on the real binary and real Postgres, with Postgres cut off behind a proxy.",
        formatter_class=argparse.ArgumentDefaultsHelpFormatter,
    )
    p.add_argument("-v", "--verbose", action="store_true", help="log DEBUG lines too")
    p.add_argument("--bin", required=True, help="the memexvault binary")
    p.add_argument("--admin-url", required=True, metavar="URL", help="a superuser's URL to any database on the Postgres server")
    p.add_argument("--data-dir", required=True, metavar="DIR", help="a folder of your own, emptied first")
    p.add_argument("--models-dir", required=True, metavar="DIR", help="the installed models")
    p.add_argument("--port", required=True, type=int, help="the server's port; the proxy takes the next one")
    return p


def main() -> int:
    args = build_parser().parse_args()
    setup_logging(args.verbose)
    try:
        report = main_run(args)
    except ScriptFailure as e:
        log.critical(str(e))
        return 1
    print(report.model_dump_json(indent=2))
    problems = failures(report)
    for p in problems:
        log.critical(p)
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
