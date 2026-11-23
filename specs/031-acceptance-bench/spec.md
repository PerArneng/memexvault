---
id: 031
name: acceptance-bench
description: Run the whole backend end to end at the test scale and measure every target
---

# 031 acceptance-bench

## Goal
Prove the backend meets the requirements as a whole. Every target in `docs/architecture.md` is measured at 200k segments and projected to 1M, and
one scripted run exercises the full path from upload to audit.

## Background
**Depends on:** all of 002-030 except the dropped 010, plus 032 and 082. Design: `docs/architecture.md`, sections Requirements, Targets.

## Approach
- A benchmark harness in the repo with a reproducible corpus generator built on public text, and scripts that take the segment count as a parameter, so spec 033 reuses them.
- An end-to-end scenario through the public APIs: upload a digital PDF, a scanned PDF and a recording. Extract, review and commit.
  Create a page and a new entity type. Search by text, identifier and type. Follow a result to a highlighted region and a time
  range. Run an audit. Then erase a source.
- Measure all target metrics, including during ingest, and cold and warm.
- A requirements checklist with evidence for each item, using the default build, with Postgres as the only external
  process. Verify using `lsof` and `otool -L`.

## Verification
1. A results table: target, measured value, pass or fail, for every row of the Targets section.
2. The scenario passes end to end and can be re-run with one command.
3. Requirement checklist complete, each with a link to a test or measurement.
4. With the LLM endpoint down: search, reading, upload and OCR still work, and extraction waits. With Postgres down: requests
   return 503, no acknowledged data is lost, and service recovers without a restart.
5. `otool -L` on the release binary lists system libraries only, and `lsof` shows no connection except Postgres and the configured
   LLM endpoint.
6. Any failed target becomes a follow-up spec, or a target revision with a recorded reason and the user's OK.

## Out of scope
New features, load beyond one machine, and a security audit by a third party.

## Result

Measured on Linux x86_64 (`Intel Core i9-14900K`, 24 cores, 62 GiB RAM, Postgres 18 + pgvector 0.8.7 in `podman`) with the release `memexvault` binary and local models (`multilingual-e5-small`, `mmarco-mMiniLMv2-L12-H384-v1` int8, `oar-ocr` PP-OCRv6/v5, `whisper-rs` `ggml-large-v3-turbo-q5_0` + Silero VAD).

### Harness & reproducibility

- **End-to-end acceptance scenario, failure modes & isolation:**
  `uv run specs/031-acceptance-bench/tools/acceptance_e2e.py --bin target/release/memexvault --admin-url <superuser-url> --data-dir <dir> --models-dir <models> --port 18031`
- **Parameterized scale benchmark (reused by spec 033 with `--segments 1000000`):**
  `cargo build -p memexvault --release --example scale_bench`
  `uv run specs/031-acceptance-bench/tools/scale_bench.py --bin target/release/memexvault --loader-bin target/release/examples/scale_bench --admin-url <superuser-url> --data-dir <dir> --models-dir <models> --segments 50000,200000 --port 18032 --pg-container <container> --container-engine podman`
- The reproducible corpus generator lives in `crates/memexvault/examples/scale_bench.rs` and builds `N` deterministic multi-language segments from the committed CC BY-SA Wikipedia fixture (`crates/memexvault/tests/fixtures/extraction/documents.jsonl`) with real `multilingual-e5-small` base embeddings and 500 `Product` entities with 32-hex `ObjectId`s.

### 1. Targets results table (50k → 200k → 1M projection)

All 11 rows of `docs/architecture.md` Targets pass at 200k segments, and every 1M power-law projection also stays within target.

| Metric | Target | 50k warm | 200k cold | 200k warm | 1M projection | Pass / Fail |
|---|---:|---:|---:|---:|---:|---|
| Exact lookup p95 | ≤2 ms | 1.23 ms | 1.61 ms | 0.99 ms | 0.99 ms | **PASS** |
| Lexical search p95 | ≤15 ms | 4.18 ms | 8.67 ms | 6.35 ms | 10.32 ms | **PASS** |
| Hybrid without rerank p95 (1 client) | ≤50 ms | 14.02 ms | 25.36 ms | 17.93 ms | 23.86 ms | **PASS** |
| Hybrid without rerank p95 (8 clients) | ≤50 ms | 40.81 ms | — | 42.55 ms | 44.66 ms | **PASS** |
| Hybrid with rerank-20 p95 (opt-in) | ≤300 ms | 106.00 ms | — | 110.59 ms | 116.17 ms | **PASS** |
| Cached query p95 | ≤1 ms | 0.59 ms | — | 0.55 ms | 0.55 ms | **PASS** |
| Search p95 during ingest | ≤1.5× idle | 1.12× (15.75 vs 14.02 ms) | — | 1.11× (19.91 vs 17.93 ms) | — | **PASS** |
| Group commit | ≥1k events/s | 13,846 ev/s | — | 14,522 ev/s | — | **PASS** |
| Restart without models, Postgres already up | ≤3 s | 1.68 s (0.02 s excl. embedder) | 1.70 s | 1.72 s (0.06 s excl. embedder) | 1.76 s | **PASS** |
| Lexical rebuild | ≤15 min | 0.65 s | — | 2.68 s | 13.88 s | **PASS** |
| memexvault RSS excl. models (Postgres recorded separately) | ≤4 GB | 1.71 GB startup / 2.39 GB w/ models (PG: 1.04 GB) | — | 1.73 GB startup / 2.43 GB w/ models (PG: 1.07 GB) | — | **PASS** |
| Machine claims with resolvable anchors | 100% | — | — | 100% (7/7: PDF rects + audio time ranges) | — | **PASS** |

- **Disk usage at 200k segments:** 931.9 MB total (Postgres: 884.0 MB, Tantivy: 47.8 MB; 244.4 MB at 50k; projected ~4.41 GB at 1M) — well within the 20 GB test-scale cap.
- **Note on hardware:** Measured on Linux x86_64 i9-14900K (62 GiB) rather than M1 Max (32 GiB). Spec 062 tracks the M1 Max 8-client hybrid latency review (`81 ms` on M1 Max in spec 020 vs `42.55 ms` here).

### 2. End-to-end scenario (`acceptance_e2e.py`, 51 s total)

1. **Upload & rendition:** Digital PDF (`two_column_en.pdf`, 8 segments), scanned PDF (`mixed_text_image.pdf`, 6 segments via `oar-ocr`), and speech recording (`speech.wav`, 1 segment via `whisper-rs`).
2. **Extract, review, commit & anchors:** Stand-in OpenAI-compatible LLM extracted 7 proposals; all 7 approved via `/changesets/{id}/approve`; **7/7 (100.0%)** machine claims resolved to non-broken anchors (PDF line rects and audio `t_start_ms`/`t_end_ms`).
3. **Wiki page & runtime entity type:** Created page `Acceptance Overview` with `[[Inspection Notes]]` link and `{{claim:...}}` ref; defined runtime `Valve` entity type and created `VLV-0042`.
4. **Search & MCP:** Hybrid+rerank (20 hits), exact identifier (`VLV-0042`), typed entity filter (`type=Valve&filter=pressure_bar:gte:40`), and MCP JSON-RPC `initialize` + `tools/call` `search` all succeeded.
5. **Provenance:** PDF `highlight.png` returned valid PNG (1,208,596 bytes); audio `/transcript` returned timed parts and `/stream` returned `206 Partial Content` (1024 bytes) for `Range: bytes=0-1023`.
6. **Audit:** On-demand audit run finished (`sampled=10`, `passed=3`, `suspect=7`, `failed=0`, `llm_calls=14`).
7. **Erasure:** Proposed and approved erasure of scanned PDF; `erase` job completed and `GET /sources/{sid}/file` returned `404`.

### 3. Failure modes & isolation

- **LLM endpoint down:** With the stand-in LLM stopped, search, page reads, upload, and OCR (6 segments) all succeeded; the new document's `extract` job stayed `queued` (`waiting`, 0 `dead`).
- **Postgres down & recovery:** With the TCP proxy cut, search returned `503` and page writes returned `503`. After restoring the proxy, the previously acknowledged page (`Before Outage`) remained intact and a new page write (`After Outage`) succeeded **without restarting `memexvault serve`**.
- **Binary libraries (`ldd` on Linux x86_64; script supports `otool -L` on macOS):** Only system libraries (`linux-vdso.so.1`, `libstdc++.so.6`, `libgcc_s.so.1`, `libm.so.6`, `libc.so.6`, `/lib64/ld-linux-x86-64.so.2`). `libpdfium.so` is dynamically loaded via `dlopen` in worker processes per spec 091.
- **Network connections (`lsof -nP -i -a -p <pid>`):** Loopback only (`127.0.0.1:18031` HTTP listener and `127.0.0.1` Postgres proxy connections); zero unexpected external sockets.

### 4. Requirements checklist (`docs/architecture.md`)

| Requirement | Evidence |
|---|---|
| Knowledge base for AI agents with HTTP API, MCP, and web console | HTTP + MCP verified in `acceptance_e2e.py`, `crates/memex-http/tests/`, `crates/memexvault/tests/mcp.rs`; console tracked in specs 040–051 |
| Live view of processing and chat with agent over AG-UI | `crates/memexvault/tests/activity.rs`, `crates/memexvault/tests/chat.rs`, `crates/memexvault/tests/chat_adk.rs`, specs 038, 041, 049, 050 |
| Three linked kinds of knowledge (wiki pages, extracted claims, structured entities) | `acceptance_e2e.py` (page with `[[link]]` + `{{claim:...}}`, extracted claims, `Valve` entity), `crates/memexvault/tests/{pages,entities,extraction}.rs` |
| Everything traceable to original document or recording with highlights / time ranges | `acceptance_e2e.py` (100% resolvable anchors, `highlight.png`, `/transcript`, `/stream` `206`), `crates/memexvault/tests/{pdf_pipeline,meeting}.rs` |
| OCR of PDFs and very fast hybrid search | `acceptance_e2e.py` (scanned PDF OCR via `oar-ocr`), `scale_bench.py` (lexical 6.35 ms, hybrid 17.93 ms p95 at 200k), `crates/memexvault/tests/golden.rs` |
| Ingestion pipeline with AI extraction, review gate, and random audit agents | `acceptance_e2e.py` (extract → review → approve → audit), `crates/memexvault/tests/{extraction,review,audit}.rs` |
| Built-in + runtime-defined searchable entity types | `acceptance_e2e.py` (runtime `Valve` type + typed filter search), `crates/memexvault/tests/entities.rs` |
| Postgres (+ pgvector) as only required external system, blobs on disk, configurable caches | `acceptance_e2e.py` (`ldd` + `lsof`), `crates/memexvault/tests/cache_tiers.rs`, `crates/memex-blob-fs/` |
| Swappable backends behind traits | `crates/memex-core/src/` traits (`Store`, `BlobStore`, `LexicalIndex`, `VectorIndex`, `PdfParser`, `Rasterizer`, `Ocr`, `Embedder`, `Reranker`, `Transcriber`, `AgentRunner`) with in-memory fakes and contract suites |
