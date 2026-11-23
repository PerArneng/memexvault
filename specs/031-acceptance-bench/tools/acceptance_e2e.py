#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = [
#     "psycopg[binary]>=3.2",
# ]
# ///
"""Spec 031 end-to-end acceptance scenario, failure-mode checks, and isolation checks.

Runs the full backend path through the real `memexvault` release binary and real Postgres:
  1. Upload a digital PDF, a scanned PDF (OCR), and a speech recording (Whisper ASR).
  2. Wait for rendition, OCR, transcription, and LLM extraction (against a local stand-in LLM).
  3. Verify 100% of machine claims have resolvable anchors (PDF rects and audio time ranges).
  4. Review and approve proposals; create a wiki page with a link and claim ref; create a runtime
     entity type and an entity of that type.
  5. Search by text (lexical, hybrid, rerank), exact identifier, entity type+filter, and via MCP.
  6. Follow a PDF result to a highlighted PNG and a recording result to a playable time range + Range stream.
  7. Run an on-demand audit and verify deterministic + LLM checks complete.
  8. Erase a source and verify its content and file are gone.
  9. Stop the LLM endpoint: verify search, reading, upload and OCR still work, and extraction waits.
 10. Cut Postgres via TCP proxy: verify 503 responses, zero loss of acknowledged data, and recovery without restart.
 11. Verify `ldd`/`otool -L` on the binary and `lsof` on the running server.

Outputs a JSON report to stdout and structured logs to stderr.
"""

from __future__ import annotations

import argparse
import http.server
import json
import logging
import os
import platform
import re
import shutil
import signal
import socket
import socketserver
import subprocess
import sys
import tempfile
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from typing import Any

import psycopg

log = logging.getLogger("acceptance_e2e")
logging.basicConfig(
    level=logging.INFO,
    format="%(asctime)s %(levelname)s %(message)s",
    datefmt="%Y-%m-%d %H:%M:%S",
    stream=sys.stderr,
)


# --------------------------------------------------------------------------- TCP proxy to Postgres


class PostgresProxy:
    """Cuttable TCP proxy in front of Postgres so we can simulate Postgres going down cleanly."""

    def __init__(self, target_host: str, target_port: int) -> None:
        self.target_host = target_host
        self.target_port = target_port
        self.cut = False
        self._lock = threading.Lock()
        self._active: list[socket.socket] = []
        self._stop = False
        self._srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self._srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self._srv.bind(("127.0.0.1", 0))
        self.port = self._srv.getsockname()[1]
        self._srv.listen(128)
        self._srv.settimeout(0.2)
        self._thread = threading.Thread(target=self._accept_loop, daemon=True)
        self._thread.start()

    def _accept_loop(self) -> None:
        while not self._stop:
            try:
                client, _ = self._srv.accept()
            except socket.timeout:
                continue
            except OSError:
                break
            if self.cut:
                client.close()
                continue
            try:
                upstream = socket.create_connection((self.target_host, self.target_port), timeout=5.0)
                upstream.settimeout(None)
                client.settimeout(None)
            except OSError:
                client.close()
                continue
            with self._lock:
                self._active.extend([client, upstream])
            threading.Thread(target=self._pipe, args=(client, upstream), daemon=True).start()
            threading.Thread(target=self._pipe, args=(upstream, client), daemon=True).start()

    def _pipe(self, src: socket.socket, dst: socket.socket) -> None:
        try:
            while not self._stop and not self.cut:
                data = src.recv(65536)
                if not data:
                    break
                dst.sendall(data)
        except OSError:
            pass
        finally:
            for s in (src, dst):
                try:
                    s.close()
                except OSError:
                    pass

    def set_cut(self, cut: bool) -> None:
        self.cut = cut
        if cut:
            with self._lock:
                for s in self._active:
                    try:
                        s.shutdown(socket.SHUT_RDWR)
                    except OSError:
                        pass
                    try:
                        s.close()
                    except OSError:
                        pass
                self._active.clear()

    def close(self) -> None:
        self._stop = True
        self.set_cut(True)
        try:
            self._srv.close()
        except OSError:
            pass


# --------------------------------------------------------------------------- Local stand-in OpenAI LLM


class StandInLlm:
    """Local OpenAI-compatible SSE stand-in for extraction, review, and audit LLM calls."""

    def __init__(self) -> None:
        self.calls = 0
        self.extraction_calls = 0
        self.entailment_calls = 0
        self.review_calls = 0
        self._httpd: _ThreadingHTTPServer | None = None
        self.port = 0

    def start(self) -> str:
        parent = self

        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, format: str, *args: Any) -> None:
                pass

            def do_POST(self) -> None:
                length = int(self.headers.get("Content-Length", "0"))
                raw = self.rfile.read(length).decode("utf-8", errors="replace")
                parent.calls += 1
                answer = parent._reply(raw)
                body = parent._sse(answer).encode("utf-8")
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream")
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)

        self._httpd = _ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.port = self._httpd.server_address[1]
        threading.Thread(target=self._httpd.serve_forever, daemon=True).start()
        return f"http://127.0.0.1:{self.port}/v1"

    def stop(self) -> None:
        if self._httpd is not None:
            self._httpd.shutdown()
            self._httpd.server_close()
            self._httpd = None

    def _sse(self, answer: str) -> str:
        def chunk(delta: dict[str, Any], finish: str | None, usage: dict[str, int] | None) -> str:
            payload = {
                "id": "standin-1",
                "object": "chat.completion.chunk",
                "created": 1,
                "model": "stand-in",
                "choices": [{"index": 0, "delta": delta, "finish_reason": finish}],
                "usage": usage,
            }
            return f"data: {json.dumps(payload)}\n\n"

        return (
            chunk({"role": "assistant", "content": ""}, None, None)
            + chunk({"content": answer}, None, None)
            + chunk({}, "stop", {"prompt_tokens": 48, "completion_tokens": 16, "total_tokens": 64})
            + "data: [DONE]\n\n"
        )

    def _reply(self, raw: str) -> str:
        if "not_entailed" in raw:
            self.entailment_calls += 1
            return json.dumps({"verdict": "entailed", "reason": "The verbatim quote supports the claim."})
        if '"abstain"' in raw and '"approve"' in raw and '"reject"' in raw:
            self.review_calls += 1
            return json.dumps({"verdict": "approve", "reason": "Quote verified against segment."})
        self.extraction_calls += 1
        return json.dumps({"claims": self._claims_from(raw), "entities": {}})

    def _claims_from(self, raw: str) -> list[dict[str, Any]]:
        try:
            req = json.loads(raw)
        except json.JSONDecodeError:
            return []
        for msg in reversed(req.get("messages", [])):
            content = msg.get("content", "")
            if not isinstance(content, str):
                content = json.dumps(content)
            unwrapped = content if '{"segments"' in content else content.replace('\\"', '"')
            idx = unwrapped.find('{"segments"')
            if idx < 0:
                continue
            segs = self._segments_in(unwrapped[idx:])
            if not segs:
                return []
            first = segs[0]
            text = first.get("text", "")
            # Pick a 4-word contiguous verbatim span from the segment text
            m = re.search(r"\S+(?:\s+\S+){1,4}", text)
            if not m:
                continue
            quote = m.group(0)
            if quote in text:
                return [
                    {
                        "segment_id": first["id"],
                        "quote": quote,
                        "subject": f"source-{self.extraction_calls}-{first['id']}",
                        "predicate": "states",
                        "value": quote,
                    }
                ]
        return []

    def _segments_in(self, text: str) -> list[dict[str, Any]]:
        end = text.rfind("}")
        while end > 0:
            try:
                parsed = json.loads(text[: end + 1])
                if isinstance(parsed, dict) and isinstance(parsed.get("segments"), list):
                    return parsed["segments"]
            except json.JSONDecodeError:
                pass
            end = text.rfind("}", 0, end)
        return []


class _ThreadingHTTPServer(socketserver.ThreadingMixIn, http.server.HTTPServer):
    daemon_threads = True
    allow_reuse_address = True


# --------------------------------------------------------------------------- HTTP helper


class ApiClient:
    def __init__(self, base_url: str, token: str) -> None:
        self.base_url = base_url.rstrip("/")
        self.token = token

    def request(
        self,
        method: str,
        path: str,
        *,
        json_body: Any = None,
        raw_body: bytes | None = None,
        content_type: str | None = None,
        extra_headers: dict[str, str] | None = None,
        timeout: float = 30.0,
    ) -> tuple[int, dict[str, str], bytes]:
        url = f"{self.base_url}{path}"
        headers = {"Authorization": f"Bearer {self.token}"}
        data: bytes | None = None
        if json_body is not None:
            data = json.dumps(json_body).encode("utf-8")
            headers["Content-Type"] = "application/json"
        elif raw_body is not None:
            data = raw_body
            if content_type:
                headers["Content-Type"] = content_type
        if extra_headers:
            headers.update(extra_headers)
        req = urllib.request.Request(url, data=data, headers=headers, method=method)
        try:
            with urllib.request.urlopen(req, timeout=timeout) as resp:
                return resp.status, dict(resp.headers.items()), resp.read()
        except urllib.error.HTTPError as e:
            return e.code, dict(e.headers.items()), e.read()

    def json(
        self,
        method: str,
        path: str,
        *,
        json_body: Any = None,
        raw_body: bytes | None = None,
        content_type: str | None = None,
        expect: int | tuple[int, ...] = (200, 201),
        timeout: float = 30.0,
    ) -> Any:
        code, _, body = self.request(
            method, path, json_body=json_body, raw_body=raw_body, content_type=content_type, timeout=timeout
        )
        ok = (expect,) if isinstance(expect, int) else expect
        if code not in ok:
            raise RuntimeError(f"{method} {path} -> HTTP {code}: {body.decode('utf-8', errors='replace')[:500]}")
        return json.loads(body.decode("utf-8")) if body else None


# --------------------------------------------------------------------------- Helpers


def synthesize_speech_wav(out_path: Path) -> Path:
    """Synthesizes a short clear English speech WAV using local cached Piper voice."""
    voice = Path.home() / ".cache" / "piper" / "en_GB-alba-medium.onnx"
    if not voice.exists():
        raise RuntimeError(f"no --audio given and cached piper voice not found at {voice}")
    script = (
        "import wave, sys; from piper import PiperVoice; "
        f"v = PiperVoice.load({str(voice)!r}); "
        f"w = wave.open({str(out_path)!r}, 'wb'); "
        "v.synthesize_wav('The turbine pressure valve in Gothenburg passed inspection on Tuesday morning. "
        "Maintenance engineers recorded a nominal reading of forty two bar.', w); w.close()"
    )
    res = subprocess.run(
        ["uv", "run", "--with", "piper-tts", "python3", "-c", script],
        capture_output=True,
        text=True,
        check=False,
    )
    if res.returncode != 0 or not out_path.exists() or out_path.stat().st_size < 1000:
        raise RuntimeError(f"failed to synthesize speech WAV: {res.stderr}")
    return out_path


def wait_for_health(base_url: str, timeout: float = 30.0) -> float:
    t0 = time.monotonic()
    deadline = t0 + timeout
    while time.monotonic() < deadline:
        try:
            with urllib.request.urlopen(f"{base_url}/api/v1/health", timeout=2.0) as r:
                if r.status == 200:
                    return time.monotonic() - t0
        except Exception:
            pass
        time.sleep(0.05)
    raise RuntimeError("server did not become healthy in time")


def wait_for_rendition(api: ApiClient, vault: str, source_id: str, timeout: float = 180.0) -> dict[str, Any]:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        data = api.json("GET", f"/api/v1/vaults/{vault}/sources/{source_id}/renditions", expect=200)
        for item in data.get("items", []):
            if not item.get("superseded_by"):
                return item
        time.sleep(0.4)
    raise RuntimeError(f"timed out waiting for rendition of {source_id}")


def wait_for_jobs_done(api: ApiClient, vault: str, kinds: set[str], timeout: float = 180.0) -> list[dict[str, Any]]:
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        jobs = api.json("GET", f"/api/v1/vaults/{vault}/jobs?limit=200", expect=200)["items"]
        relevant = [j for j in jobs if j["kind"] in kinds]
        if relevant and all(j["state"] in ("done", "dead", "cancelled") for j in relevant):
            failed = [j for j in relevant if j["state"] != "done"]
            if failed:
                raise RuntimeError(f"jobs did not succeed: {failed}")
            return relevant
        time.sleep(0.4)
    raise RuntimeError(f"timed out waiting for jobs {kinds}")


def check_binary_libraries(bin_path: Path) -> dict[str, Any]:
    system = platform.system()
    if system == "Darwin":
        out = subprocess.check_output(["otool", "-L", str(bin_path)], text=True)
        libs = [line.strip().split(" ")[0] for line in out.splitlines()[1:] if line.strip()]
        unexpected = [lib for lib in libs if not (lib.startswith("/usr/lib/") or lib.startswith("/System/Library/"))]
        return {"tool": "otool -L", "libraries": libs, "unexpected": unexpected, "ok": len(unexpected) == 0}
    else:
        out = subprocess.check_output(["ldd", str(bin_path)], text=True)
        libs = [line.strip() for line in out.splitlines() if line.strip()]
        allowed_prefixes = (
            "linux-vdso.so",
            "libgcc_s.so",
            "libm.so",
            "libc.so",
            "libpthread.so",
            "libdl.so",
            "librt.so",
            "libstdc++.so",
            "ld-linux",
            "/lib64/ld-linux",
            "libpdfium.so",  # spec 091: dynamically loaded pinned pdfium on Linux x86_64
        )
        unexpected = []
        for lib in libs:
            name = lib.split()[0]
            if not any(name.startswith(p) for p in allowed_prefixes):
                unexpected.append(lib)
        return {"tool": "ldd", "libraries": libs, "unexpected": unexpected, "ok": len(unexpected) == 0}


def check_lsof_network(pid: int, allowed_ports: set[int]) -> dict[str, Any]:
    res = subprocess.run(["lsof", "-nP", "-i", "-a", "-p", str(pid)], capture_output=True, text=True, check=False)
    lines = [l for l in res.stdout.splitlines()[1:] if l.strip()]
    unexpected = []
    for line in lines:
        # Must only talk to loopback 127.0.0.1 or [::1]
        if "127.0.0.1" not in line and "[::1]" not in line:
            unexpected.append(line)
    return {"lines": lines, "unexpected": unexpected, "ok": len(unexpected) == 0}


# --------------------------------------------------------------------------- Main scenario


def main() -> int:
    ap = argparse.ArgumentParser(description="Spec 031 end-to-end acceptance scenario")
    ap.add_argument("--bin", required=True, type=Path, help="Path to memexvault release binary")
    ap.add_argument("--admin-url", required=True, help="Superuser Postgres URL")
    ap.add_argument("--data-dir", required=True, type=Path, help="Data dir (emptied first)")
    ap.add_argument("--models-dir", required=True, type=Path, help="Installed models directory")
    ap.add_argument("--port", type=int, default=18031, help="HTTP port for memexvault serve")
    ap.add_argument("--audio", type=Path, default=None, help="Optional speech WAV/MP3 path")
    ap.add_argument("--keep-db", action="store_true", help="Do not drop the test database afterwards")
    args = ap.parse_args()

    repo_root = Path(__file__).resolve().parents[3]
    pdf_fixtures = repo_root / "crates" / "memex-pdf" / "tests" / "fixtures"
    digital_pdf = pdf_fixtures / "two_column_en.pdf"
    scanned_pdf = pdf_fixtures / "mixed_text_image.pdf"

    parsed = urllib.parse.urlparse(args.admin_url)
    pg_host = parsed.hostname or "127.0.0.1"
    pg_port = parsed.port or 5432
    db_name = f"memex_s031_e2e_{os.getpid()}"

    if args.data_dir.exists():
        shutil.rmtree(args.data_dir)
    args.data_dir.mkdir(parents=True, exist_ok=True)

    audio_path = args.audio
    tmp_audio: Path | None = None
    if audio_path is None:
        tmp_audio = args.data_dir / "speech.wav"
        log.info("synthesizing speech recording with cached piper voice")
        audio_path = synthesize_speech_wav(tmp_audio)

    # Create fresh database
    with psycopg.connect(args.admin_url, autocommit=True) as conn:
        conn.execute(f"DROP DATABASE IF EXISTS {db_name}")
        conn.execute(f"CREATE DATABASE {db_name}")

    proxy = PostgresProxy(pg_host, pg_port)
    llm = StandInLlm()
    llm_url = llm.start()
    server_proc: subprocess.Popen[str] | None = None
    report: dict[str, Any] = {"steps": {}}

    try:
        proxied_url = parsed._replace(netloc=f"{parsed.username}:{parsed.password}@127.0.0.1:{proxy.port}", path=f"/{db_name}").geturl()
        direct_url = parsed._replace(path=f"/{db_name}").geturl()

        env = os.environ.copy()
        env.update(
            {
                "MEMEX_DATABASE_URL": proxied_url,
                "MEMEX_DATA_DIR": str(args.data_dir),
                "MEMEX_MODELS_DIR": str(args.models_dir),
                "MEMEX_HTTP_BIND": f"127.0.0.1:{args.port}",
                "MEMEX_LLM_PROVIDER": "openai",
                "MEMEX_LLM_BASE_URL": llm_url,
                "MEMEX_LLM_MODEL": "stand-in",
                "MEMEX_LLM_API_KEY": "stand-in-key",
                "MEMEX_REVIEW_MODEL": "stand-in",
                "MEMEX_AUDIT_MODEL": "stand-in",
            }
        )

        # Binary library check
        log.info("checking binary dynamic libraries")
        lib_check = check_binary_libraries(args.bin)
        report["binary_libraries"] = lib_check
        if not lib_check["ok"]:
            raise RuntimeError(f"unexpected shared libraries: {lib_check['unexpected']}")

        # Migrate & bootstrap
        log.info("running migrate and bootstrap")
        subprocess.run([str(args.bin), "migrate"], env=env, check=True, capture_output=True, text=True)
        boot = subprocess.run([str(args.bin), "bootstrap", "acceptance-admin"], env=env, check=True, capture_output=True, text=True)
        token = boot.stdout.strip().splitlines()[-1].strip()

        # Start server
        log.info("starting memexvault serve on port %d", args.port)
        server_log = open(args.data_dir / "server.log", "w")
        server_proc = subprocess.Popen([str(args.bin), "serve"], env=env, stdout=server_log, stderr=subprocess.STDOUT, text=True)
        base_url = f"http://127.0.0.1:{args.port}"
        startup_s = wait_for_health(base_url, timeout=30.0)
        report["startup_s"] = round(startup_s, 3)
        api = ApiClient(base_url, token)

        # Create vault
        vault = api.json("POST", "/api/v1/vaults", json_body={"name": "acceptance-e2e"})["id"]
        log.info("created vault %s", vault)

        # 1. Upload digital PDF, scanned PDF, and recording
        log.info("uploading digital PDF, scanned PDF, and speech recording")
        src_digital = api.json(
            "POST",
            f"/api/v1/vaults/{vault}/sources?name=digital.pdf&trust_tier=trusted",
            raw_body=digital_pdf.read_bytes(),
            content_type="application/pdf",
        )["source"]["id"]
        src_scanned = api.json(
            "POST",
            f"/api/v1/vaults/{vault}/sources?name=scanned.pdf&trust_tier=trusted",
            raw_body=scanned_pdf.read_bytes(),
            content_type="application/pdf",
        )["source"]["id"]
        src_audio = api.json(
            "POST",
            f"/api/v1/vaults/{vault}/sources?name=meeting.wav&trust_tier=trusted",
            raw_body=audio_path.read_bytes(),
            content_type="audio/wav",
        )["source"]["id"]

        ren_digital = wait_for_rendition(api, vault, src_digital)
        ren_scanned = wait_for_rendition(api, vault, src_scanned)
        ren_audio = wait_for_rendition(api, vault, src_audio)
        log.info(
            "renditions ready digital=%d scanned=%d audio=%d segments",
            ren_digital["segment_count"],
            ren_scanned["segment_count"],
            ren_audio["segment_count"],
        )
        assert ren_digital["segment_count"] > 0
        assert ren_scanned["segment_count"] > 0
        assert ren_audio["segment_count"] > 0
        report["steps"]["upload_and_rendition"] = {
            "digital_segments": ren_digital["segment_count"],
            "scanned_segments": ren_scanned["segment_count"],
            "audio_segments": ren_audio["segment_count"],
        }

        # 2. Wait for extraction, review, and embed jobs
        log.info("waiting for extraction, review, and embedding jobs")
        wait_for_jobs_done(api, vault, {"extract", "embed"}, timeout=180.0)
        # Wait briefly for reviewer jobs if enqueued
        time.sleep(1.0)
        try:
            wait_for_jobs_done(api, vault, {"review"}, timeout=60.0)
        except RuntimeError:
            pass

        # 3. Inspect review queue, approve all machine proposals, verify 100% resolvable anchors
        queue = api.json("GET", f"/api/v1/vaults/{vault}/review/queue?state=pending&limit=200", expect=200)["items"]
        log.info("pending review items: %d", len(queue))
        assert len(queue) >= 3, f"expected extracted proposals from all 3 sources, got {len(queue)}"

        approved_claims: list[str] = []
        for item in queue:
            cs_id = item["changeset"]["id"]
            api.json("POST", f"/api/v1/vaults/{vault}/changesets/{cs_id}/approve", json_body={"reason": "acceptance e2e"}, expect=200)
            for op in item["changeset"].get("ops", []):
                if op.get("op") == "add_assertion":
                    approved_claims.append(op["id"])

        # Verify 100% of machine claims have resolvable anchors
        resolvable = 0
        pdf_claim_id = None
        pdf_ev_id = None
        audio_claim_id = None
        for cid in approved_claims:
            lin = api.json("GET", f"/api/v1/vaults/{vault}/assertions/{cid}/lineage", expect=200)
            evs = lin.get("evidence", [])
            assert len(evs) > 0, f"claim {cid} has no evidence"
            for ev in evs:
                if not ev.get("broken") and ev.get("quote"):
                    if ev.get("rects"):
                        resolvable += 1
                        if pdf_claim_id is None:
                            pdf_claim_id = cid
                            pdf_ev_id = ev["evidence_id"]
                    elif ev.get("t_start_ms") is not None and ev.get("t_end_ms") is not None:
                        resolvable += 1
                        if audio_claim_id is None:
                            audio_claim_id = cid
        anchor_rate = resolvable / len(approved_claims) if approved_claims else 0.0
        log.info("machine claims=%d resolvable_anchors=%d (%.1f%%)", len(approved_claims), resolvable, anchor_rate * 100.0)
        assert resolvable == len(approved_claims) and len(approved_claims) > 0
        assert pdf_claim_id is not None and audio_claim_id is not None
        report["steps"]["extract_review_anchors"] = {
            "machine_claims": len(approved_claims),
            "resolvable_anchors": resolvable,
            "anchor_rate_pct": round(anchor_rate * 100.0, 2),
        }

        # 4. Create a wiki page with a link and claim ref, plus a new runtime entity type and entity
        log.info("creating wiki page and runtime entity type")
        page_res = api.json(
            "POST",
            f"/api/v1/vaults/{vault}/pages",
            json_body={
                "title": "Acceptance Overview",
                "slug": "acceptance-overview",
                "body": f"# Acceptance Overview\n\nSee [[Inspection Notes]] and claim {{{{claim:{pdf_claim_id}}}}}.",
                "rationale": "acceptance scenario",
                "idempotency_key": "e2e-page-1",
            },
            expect=200,
        )
        page_id = page_res["page_id"]

        api.json(
            "POST",
            f"/api/v1/vaults/{vault}/entity-types",
            json_body={
                "name": "Valve",
                "base_version": None,
                "schema": {
                    "$schema": "https://json-schema.org/draft/2020-12/schema",
                    "type": "object",
                    "additionalProperties": False,
                    "required": ["code"],
                    "properties": {
                        "code": {"type": "string", "x-memex-identity": True, "x-memex-searchable": True, "x-memex-filterable": True},
                        "pressure_bar": {"type": "number", "x-memex-filterable": True},
                        "location": {"type": "string", "x-memex-searchable": True, "x-memex-filterable": True},
                    },
                },
                "rationale": "define Valve runtime type",
                "idempotency_key": "e2e-type-valve",
            },
            expect=200,
        )
        ent_res = api.json(
            "POST",
            f"/api/v1/vaults/{vault}/entities",
            json_body={
                "type": "Valve",
                "aliases": ["VLV-0042", "Gothenburg Main Valve"],
                "fields": [
                    {"field": "code", "value": "VLV-0042", "evidence": []},
                    {"field": "pressure_bar", "value": 42, "evidence": []},
                    {"field": "location", "value": "Gothenburg", "evidence": []},
                ],
                "rationale": "create Valve entity",
                "idempotency_key": "e2e-entity-valve-1",
            },
            expect=200,
        )
        entity_id = ent_res["entity_id"]
        report["steps"]["page_and_entity"] = {"page_id": page_id, "entity_id": entity_id}

        # Wait for lexical/vector projectors to catch up
        time.sleep(1.5)

        # 5. Search by text, identifier, type+filter, and via MCP
        log.info("running searches (text, identifier, entity type, MCP)")
        s_hybrid = api.json("GET", f"/api/v1/vaults/{vault}/search?q=Gothenburg+valve&mode=hybrid&rerank=true", expect=200)
        s_ident = api.json("GET", f"/api/v1/vaults/{vault}/search?q=VLV-0042", expect=200)
        s_filter = api.json("GET", f"/api/v1/vaults/{vault}/search?q=Gothenburg&type=Valve&filter=pressure_bar:gte:40", expect=200)
        assert len(s_hybrid["items"]) > 0
        assert any(it.get("id") == entity_id for it in s_ident["items"]), f"exact identifier search missed entity: {s_ident}"
        assert any(it.get("id") == entity_id for it in s_filter["items"]), f"typed filter search missed entity: {s_filter}"

        # MCP search check
        mcp_init_code, mcp_headers, _ = api.request(
            "POST",
            "/api/v1/mcp",
            json_body={
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-03-26",
                    "capabilities": {},
                    "clientInfo": {"name": "acceptance-e2e", "version": "1.0"},
                },
            },
            extra_headers={"Accept": "application/json, text/event-stream"},
        )
        assert mcp_init_code == 200, f"MCP initialize failed: {mcp_init_code}"
        session_id = mcp_headers.get("mcp-session-id") or mcp_headers.get("Mcp-Session-Id")
        mcp_call_headers = {"Accept": "application/json, text/event-stream"}
        if session_id:
            mcp_call_headers["mcp-session-id"] = session_id
        mcp_code, _, mcp_body = api.request(
            "POST",
            "/api/v1/mcp",
            json_body={
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/call",
                "params": {"name": "search", "arguments": {"vault": vault, "q": "VLV-0042"}},
            },
            extra_headers=mcp_call_headers,
        )
        assert mcp_code == 200 and b"VLV-0042" in mcp_body, f"MCP search failed: {mcp_code} {mcp_body[:300]}"
        report["steps"]["search_and_mcp"] = {
            "hybrid_hits": len(s_hybrid["items"]),
            "identifier_hit": True,
            "typed_filter_hit": True,
            "mcp_ok": True,
        }

        # 6. Provenance: highlighted PDF PNG & audio time range + Range stream
        log.info("checking provenance highlight PNG and audio Range stream")
        assert pdf_ev_id is not None
        png_code, png_hdrs, png_bytes = api.request(
            "GET", f"/api/v1/vaults/{vault}/assertions/{pdf_claim_id}/evidence/{pdf_ev_id}/highlight.png"
        )
        assert png_code == 200 and png_bytes[:8] == b"\x89PNG\r\n\x1a\n", f"highlight.png failed: {png_code}"

        transcript = api.json("GET", f"/api/v1/vaults/{vault}/sources/{src_audio}/transcript", expect=200)
        assert len(transcript.get("parts", [])) > 0
        range_code, range_hdrs, range_bytes = api.request(
            "GET",
            f"/api/v1/vaults/{vault}/sources/{src_audio}/stream",
            extra_headers={"Range": "bytes=0-1023"},
        )
        assert range_code == 206 and len(range_bytes) == 1024, f"audio stream range failed: {range_code} len={len(range_bytes)}"
        report["steps"]["provenance"] = {
            "highlight_png_bytes": len(png_bytes),
            "audio_range_status": range_code,
            "audio_range_bytes": len(range_bytes),
        }

        # 7. Run an on-demand audit
        log.info("triggering on-demand audit run")
        audit_run = api.json(
            "POST",
            f"/api/v1/vaults/{vault}/audit/runs",
            json_body={"sample_size": 10, "run_token_budget": 10000, "seed": "42"},
            expect=(200, 201, 202),
        )
        run_id = audit_run["id"]
        deadline = time.monotonic() + 120.0
        final_run = None
        while time.monotonic() < deadline:
            detail = api.json("GET", f"/api/v1/vaults/{vault}/audit/runs/{run_id}", expect=200)
            r = detail["run"]
            if r.get("state") in ("finished", "failed"):
                final_run = r
                break
            time.sleep(0.4)
        assert final_run is not None and final_run.get("state") == "finished", f"audit run did not finish cleanly: {final_run}"
        report["steps"]["audit"] = {
            "run_id": run_id,
            "state": final_run.get("state"),
            "sampled": final_run.get("sampled"),
            "passed": final_run.get("passed"),
            "suspect": final_run.get("suspect"),
            "failed": final_run.get("failed"),
            "llm_calls": final_run.get("llm_calls"),
        }

        # 8. Erase a source
        log.info("erasing scanned PDF source")
        erase_prop = api.json(
            "POST",
            f"/api/v1/vaults/{vault}/sources/{src_scanned}/erasure",
            json_body={"rationale": "acceptance erasure check"},
            expect=200,
        )
        erase_cs = erase_prop["changeset"]["id"]
        api.json("POST", f"/api/v1/vaults/{vault}/changesets/{erase_cs}/approve", json_body={"reason": "approve erase"}, expect=200)
        wait_for_jobs_done(api, vault, {"erase"}, timeout=120.0)
        file_code, _, _ = api.request("GET", f"/api/v1/vaults/{vault}/sources/{src_scanned}/file")
        assert file_code == 404, f"erased source file still returned {file_code}"
        report["steps"]["erasure"] = {"erased_source": src_scanned, "file_status_after": file_code}

        # Check lsof while server is live with Postgres + LLM
        log.info("checking lsof network connections of running server")
        lsof_check = check_lsof_network(server_proc.pid, {args.port, proxy.port, llm.port})
        report["lsof_network"] = lsof_check
        if not lsof_check["ok"]:
            raise RuntimeError(f"unexpected network connections in lsof: {lsof_check['unexpected']}")

        # 9. LLM endpoint down: search, reading, upload, OCR still work; extraction waits
        log.info("stopping stand-in LLM to verify degraded behavior")
        llm.stop()
        s_down = api.json("GET", f"/api/v1/vaults/{vault}/search?q=Gothenburg", expect=200)
        assert len(s_down["items"]) > 0
        p_down = api.json("GET", f"/api/v1/vaults/{vault}/pages/{page_id}", expect=200)
        assert p_down["title"] == "Acceptance Overview"
        src_while_down = api.json(
            "POST",
            f"/api/v1/vaults/{vault}/sources?name=scanned_llm_down.pdf&trust_tier=trusted",
            raw_body=scanned_pdf.read_bytes(),
            content_type="application/pdf",
        )["source"]["id"]
        ren_down = wait_for_rendition(api, vault, src_while_down, timeout=120.0)
        assert ren_down["segment_count"] > 0
        # Wait for the new extract job to hit the dead LLM and enter waiting/queued (not dead)
        time.sleep(2.5)
        jobs_down = api.json("GET", f"/api/v1/vaults/{vault}/jobs?limit=200", expect=200)["items"]
        ext_jobs = [j for j in jobs_down if j["kind"] == "extract" and j["state"] in ("queued", "running")]
        dead_ext = [j for j in jobs_down if j["kind"] == "extract" and j["state"] == "dead"]
        assert len(ext_jobs) >= 1 and len(dead_ext) == 0, f"expected waiting extract job, got {jobs_down}"
        report["steps"]["llm_down"] = {
            "search_ok": True,
            "read_ok": True,
            "upload_and_ocr_segments": ren_down["segment_count"],
            "extraction_waiting_jobs": len(ext_jobs),
            "extraction_dead_jobs": len(dead_ext),
        }

        # 10. Postgres down: 503, no acknowledged data lost, recovery without restart
        log.info("cutting Postgres proxy to verify 503 and recovery without restart")
        ack_page = api.json(
            "POST",
            f"/api/v1/vaults/{vault}/pages",
            json_body={
                "title": "Before Outage",
                "slug": "before-outage",
                "body": "Acknowledged write before Postgres outage.",
                "rationale": "outage durability check",
                "idempotency_key": "e2e-before-outage",
            },
            expect=200,
        )["page_id"]
        proxy.set_cut(True)
        time.sleep(0.5)
        read_503, _, _ = api.request("GET", f"/api/v1/vaults/{vault}/search?q=Gothenburg", timeout=15.0)
        write_503, _, _ = api.request(
            "POST",
            f"/api/v1/vaults/{vault}/pages",
            json_body={
                "title": "During Outage",
                "slug": "during-outage",
                "body": "Should fail with 503.",
                "rationale": "outage check",
                "idempotency_key": "e2e-during-outage",
            },
            timeout=15.0,
        )
        assert read_503 == 503, f"expected 503 on search during Postgres outage, got {read_503}"
        assert write_503 == 503, f"expected 503 on write during Postgres outage, got {write_503}"

        proxy.set_cut(False)
        # Wait for pool + writer lease to recover without restarting memexvault
        recovered = False
        for _ in range(40):
            code, _, body = api.request("GET", f"/api/v1/vaults/{vault}/pages/{ack_page}", timeout=5.0)
            if code == 200 and b"Before Outage" in body:
                w_code, _, _ = api.request(
                    "POST",
                    f"/api/v1/vaults/{vault}/pages",
                    json_body={
                        "title": "After Outage",
                        "slug": "after-outage",
                        "body": "Written after Postgres recovered without server restart.",
                        "rationale": "recovery check",
                        "idempotency_key": "e2e-after-outage",
                    },
                    timeout=5.0,
                )
                if w_code == 200:
                    recovered = True
                    break
            time.sleep(0.5)
        assert recovered, "server did not recover writes without restart after Postgres came back"
        report["steps"]["postgres_outage"] = {
            "search_status_during_outage": read_503,
            "write_status_during_outage": write_503,
            "acknowledged_data_intact": True,
            "recovered_without_restart": True,
        }

        report["ok"] = True
        print(json.dumps(report, indent=2))
        return 0
    finally:
        if server_proc is not None and server_proc.poll() is None:
            server_proc.send_signal(signal.SIGTERM)
            try:
                server_proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                server_proc.kill()
        llm.stop()
        proxy.close()
        if not args.keep_db:
            with psycopg.connect(args.admin_url, autocommit=True) as conn:
                conn.execute(
                    "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = %s AND pid <> pg_backend_pid()",
                    (db_name,),
                )
                conn.execute(f"DROP DATABASE IF EXISTS {db_name}")


if __name__ == "__main__":
    sys.exit(main())
