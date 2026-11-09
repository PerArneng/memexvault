# AGENTS.md

Guidance for AI coding agents working in this repository.

## Project

memexvault is a knowledge base, or "second brain", for AI agents.

The repository holds this file, the license, the spec-driven workflow, the console's
workspace in `web/`, the backend's Cargo workspace in `crates/`, and the specs in `specs/`, which plan
the backend and web console and record what's done.
[docs/architecture.md](docs/architecture.md) holds the backend's shared design, and
[docs/frontend-architecture.md](docs/frontend-architecture.md) holds the console's.

Decided: the backend is Rust, structured by the `rust-architecture` skill, with an
HTTP API and MCP. PostgreSQL, with pgvector allowed, is the system of record and
the one required external system. Blobs live in folders on disk. AI calls are the
only other accepted network dependency. Backends are swappable behind traits, and
code above the edge traits must not assume a particular backend. The console is
React and TypeScript, structured by the frontend document, which adapts
`rust-architecture`. The spec tooling's use of Python and `uv` doesn't decide anything for the product.

`research/` holds background reports converted from PDF (spec 001). It's git-ignored
and exists only locally, so never commit it or copy passages from it. The reports
are source material, not decisions: treat their claims as unverified.

## Start here

Run `uv run .claude/skills/gh-spec-driven/scripts/wave-manager.py next` to see which
waves can start and what to start in them, and
`uv run .claude/skills/gh-spec-driven/scripts/gh-spec-manager.py list` to see every spec.
Backend and console run as separate tracks of waves, and each console screen follows its backend spec.

## Working in this repository

- Follow `docs/architecture.md`. If a task depends on a decision it doesn't cover,
  ask rather than choosing one silently.
- Keep changes small and limited to the task at hand.
- Update this file when a decision changes what it says. As the project takes
  shape, add the real build, test, and lint commands here.

## Spec-driven development

All non-trivial work is done from a spec. That covers new features, design and
technology decisions, changes to behavior, and tooling. Fixing a typo or a small
note doesn't need a spec. The `gh-spec-driven` skill
(`.claude/skills/gh-spec-driven/SKILL.md`) has the full workflow. Its script
`.claude/skills/gh-spec-driven/scripts/gh-spec-manager.py` is the only way to create
specs and change their status, and it finds `<repo>/specs` from any directory.

Every spec is a GitHub issue. The issue number is the spec's id, and the issue holds its
status, so status is the same from every branch and worktree. `specs/<id>-<name>/` holds
only the content and never moves. The workflow needs `gh` logged in with write access to
the repo; the product doesn't depend on GitHub. The repo is public, so issues are too:
keep them to what could appear in the repo, and don't create test issues on it.

Waves are issues labelled `wave`, titled `wave: ...`, that group specs fairly safe to do in parallel and list the waves
that must finish first, so they form a graph. Work on issues from a wave that `next` offers, and don't start a blocked wave:
`wave-manager.py gate <wave>` exits 0 when it may start and 3 when it's blocked, and `--wait` polls until it opens. Each issue in a wave gets its own
worktree, and they land on `main` one at a time. Run `wave-manager.py sync` after `done`. New specs need a `Depends on` line and a
place in a wave. `wave-manager.py dot | dot -Tsvg -o waves.svg` draws the graph. The `gh-spec-driven` skill covers waves in full, and
`wave-plan.py` helps group specs into them.

Each spec's Verification section states its own concrete check. Backend commands, from
the repository root:

- First time: `scripts/fetch-pdfium.sh`, which puts the pinned static pdfium in `vendor/`.
- `cargo build --workspace`, `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo fmt --check` and `cargo deny check licenses`.
- `migrate` upgrades every vault in the database, and a binary refuses a schema newer than it knows, so a branch that adds a migration should run these against a database of its own.
- Tests that need Postgres are ignored by default, so they show as not run rather than passed: `docker compose up -d`, then
  `MEMEX_TEST_DATABASE_URL=postgres://postgres:memex@127.0.0.1:55432/memex cargo test -p memexvault --release -- --ignored --test-threads=1 pg_`.
  The `pg_kill_postgres` test also needs `MEMEX_TEST_CONTAINER=memexvault-postgres-1` and kills that container repeatedly.
- TLS to Postgres needs a second server: `scripts/pg-tls-certs.sh`, then `docker compose --profile tls up -d postgres-tls` (name the service, or the default one starts too), then
  `MEMEX_TEST_DATABASE_URL=postgres://postgres:memex@127.0.0.1:55432/memex MEMEX_TEST_TLS_CERTS=$PWD/target/pg-tls cargo test -p memexvault --release --test postgres_tls -- --ignored --test-threads=1`. The certificates are throwaway and git-ignored.
- The keyword index harness needs Postgres and the corpus from spec 004: set `MEMEX_TEST_DATABASE_URL`, `MEMEX_TEST_DATA` and `MEMEX_TEST_CORPUS`, then
  `cargo test -p memexvault --release --test lexical_harness -- --ignored --nocapture --exact lexical_bench_200k` or `lexical_kill_200_times`.
- The vector index needs Postgres: `cargo test -p memexvault --release --test vector -- --ignored --test-threads=1 pg_`. `real_model_matches_the_spike_and_is_fast_enough` in that file also needs `MEMEX_TEST_E5` (`model.onnx` and `tokenizer.json`), and `MEMEX_TEST_CORPUS` and `MEMEX_TEST_VECTORS` from spec 004. The harness uses the same variables: `cargo test -p memexvault --release --test vector_harness -- --ignored --nocapture --exact vector_bench_200k` or `vector_kill_200_times`.
- Search needs Postgres, the same data, and `MEMEX_TEST_DATA`, `MEMEX_TEST_E5` and `MEMEX_TEST_RERANKER` (spec 005's `mmarco` folder): `cargo test -p memexvault --release --test golden -- --ignored --nocapture --exact golden_recall` or `latency_50k_200k`, on the golden set in `crates/memexvault/tests/fixtures/golden/`.
- The entity filter benchmark needs Postgres and its own database: `cargo test -p memexvault --release --test entities -- --ignored --nocapture --exact entity_filter_bench`. It loads up to 1M field values, in about 6 minutes and 1 GB, and `MEMEX_ENTITY_BENCH_MAX=200000` stops earlier.
- The k-hop decision needs the same data and models as search, and about 1 GB in its own database: `cargo test -p memexvault --release --test golden -- --ignored --nocapture --exact relational_khop`, on `crates/memexvault/tests/fixtures/golden/golden_relational.jsonl`. `MEMEX_KHOP_ROUNDS` sets its latency rounds, by default 5. Spec 004's data is in `specs/004-spike-vector/{corpus,out}` and spec 005's models in `specs/005-spike-inference/models/{e5s,mmarco}`, in the main checkout.
- OCR tests are ignored unless `MEMEX_TEST_MODELS` names a folder of model files: `cargo test -p memexvault --release --test pdf_pipeline -- --ignored --test-threads=1`.
- Extraction on Postgres uses a scripted agent, so no LLM: `cargo test -p memexvault --release --test extraction -- --ignored --test-threads=1 pg_`. The labelled set and its scorer run on a scripted agent as ordinary tests in `extraction_eval`, and those numbers say nothing about a model. The live run calls an LLM, costs money and sends documents to the provider, so run it only when asked: set `MEMEX_LLM_PROVIDER`, `MEMEX_LLM_MODEL`, `MEMEX_LLM_API_KEY` or `MEMEX_LLM_BASE_URL`, `MEMEX_EVAL_APPROVED` (a file of approved document ids), `MEMEX_EVAL_MAX_TOKENS`, and optionally `MEMEX_EVAL_OUT` and `MEMEX_EVAL_ADJUDICATIONS`, then `cargo test -p memexvault --release --test extraction_eval -- --ignored --nocapture --exact live_labelled_set`.
- Extraction is off unless `MEMEX_LLM_PROVIDER` is `gemini` (needs `MEMEX_LLM_MODEL`, `MEMEX_LLM_API_KEY`) or `openai` (needs `MEMEX_LLM_MODEL`, `MEMEX_LLM_BASE_URL`). `MEMEX_EXTRACT_TOKEN_BUDGET`, `MEMEX_EXTRACT_BATCH_TOKENS`, `MEMEX_EXTRACT_OUTPUT_TOKENS` and `MEMEX_EXTRACT_TIMEOUT_S` tune it.
- Cache tiers: `MEMEX_CACHE_QUERY_BYTES`, `MEMEX_CACHE_INFERENCE_BYTES`, `MEMEX_CACHE_OBJECT_BYTES` and `MEMEX_CACHE_DISK_BYTES` set each cap in bytes, and 0 turns a tier off. `cargo test -p memexvault --release --test cache_tiers -- --nocapture caps_hold` is the load test, and `MEMEX_CACHE_LOAD_MB` sets its cap, by default 8. The replay needs what search needs: `cargo test -p memexvault --release --test golden -- --ignored --nocapture --exact cache_tiers_replay`. `MEMEX_CACHE_SEGMENTS` (by default 200000) and `MEMEX_CACHE_ROUNDS` (by default 5) tune it.
- The review gate on Postgres uses a scripted agent, so no LLM: `cargo test -p memexvault --release --test review -- --ignored --test-threads=1 pg_`. The injection fixtures run on a scripted agent as ordinary tests in `memex-core/tests/review.rs`, which prove the mechanisms and say nothing about what a model does. The live runs in `review_eval` call an LLM and cost money, so run them only when asked: set the `MEMEX_LLM_*` settings and `MEMEX_EVAL_MAX_TOKENS`, then `cargo test -p memexvault --release --test review_eval -- --ignored --nocapture --exact live_injection_fixtures` or `live_reviewer_agreement`. The latter takes `MEMEX_REVIEW_LABELS`, a file of labels in the format of `crates/memexvault/tests/fixtures/review/labelled.jsonl`, whose labels are an agent's and don't count until a person has checked them.
- The reviewer agent is off unless `MEMEX_REVIEW_MODEL` names a model, on the same provider as extraction, and each queued claim is then a call. `MEMEX_REVIEW_OUTPUT_TOKENS` and `MEMEX_REVIEW_TIMEOUT_S` tune it. `MEMEX_AUTO_COMMIT_THRESHOLD`, above 0 and at most 1 and by default 0.95, is what a combination's Wilson bound has to reach.
- Recordings on Postgres, with no model, use a word table made in the test and a scripted agent: `cargo test -p memexvault --release --test meeting -- --ignored --test-threads=1 pg_`. The real transcriber needs a database of your own, `MEMEX_TEST_ASR_MODELS` (spec 027's `models` folder), `MEMEX_TEST_ASR_CORPUS` (its `corpus` folder) and `MEMEX_TEST_ASR_BAD` (its `out/bad`): `cargo test -p memexvault --release --test meeting_eval -- --ignored --nocapture --test-threads=1 asr_`. `asr_long_quiet_and_noisy_stay_within_memory` needs a few GB of disk. The labels are a synthesizer's and the agent is scripted, and no provider is called. `MEMEX_ASR_DECIDES` names the recording item 1's assertion applies to, by default `en_60m`.
- Transcription runs on `MEMEX_MODELS_DIR/whisper`, which `memexvault models fetch` fills with 575 MB. `MEMEX_TRANSCRIBE_THREADS`, `MEMEX_TRANSCRIBE_MAX_SECONDS`, `MEMEX_TRANSCRIBE_MAX_RSS_MB` and `MEMEX_TRANSCRIBE_STALL_S` tune it. The transcription API is off unless `MEMEX_BACKEND_TRANSCRIBER` is `gemini`, which needs `MEMEX_TRANSCRIBE_API_MODEL` and `MEMEX_TRANSCRIBE_API_KEY`, and it then sends recordings to the provider. `MEMEX_TRANSCRIBE_API_BASE_URL` replaces the provider's address, and `MEMEX_TRANSCRIBE_API_MAX_TOKENS` limits one recording, by default 1,000,000. A key on its own does nothing. Its live run calls the provider and costs money, so run it only when asked: spec 028's Result has the command.
- Audits on Postgres use a scripted agent, so no LLM: `cargo test -p memexvault --release --test audit -- --ignored --test-threads=1 pg_`. `pg_audit_planted_errors_*` take minutes, as they build 172 documents. The live run calls an LLM and costs money, so run it only when asked: set the `MEMEX_LLM_*` settings and `MEMEX_EVAL_MAX_TOKENS`, then `cargo test -p memexvault --release --test audit -- --ignored --nocapture --exact live_planted_recall`. Only that run says anything about a model.
- Search latency during audits needs what search needs: `cargo test -p memexvault --release --test golden -- --ignored --nocapture --exact audit_search_latency`. `MEMEX_AUDIT_SEGMENTS` (by default 200000), `MEMEX_AUDIT_PAIRS` (by default 6) and `MEMEX_AUDIT_ROUND_S` (by default 30) tune it, and the pass rule is in spec 026.
- Audits run their deterministic checks on any schedule a vault sets, with no LLM. Their LLM checks are off unless `MEMEX_AUDIT_MODEL` names a model, on the same provider as extraction, and each sampled claim is then up to two calls. `MEMEX_AUDIT_OUTPUT_TOKENS` and `MEMEX_AUDIT_TIMEOUT_S` tune them, and budgets are on the schedule.
- Chat on Postgres uses a scripted model, so no LLM: `cargo test -p memexvault --release --test chat -- --ignored --test-threads=1 --nocapture pg_`. `pg_erasing_a_source_*` reads the chat tables' own files with `pg_read_binary_file`, so it needs a superuser, and runs `CHECKPOINT`. `--test chat_adk` runs as an ordinary test: the real ADK runner against a stand-in server on this machine. The streams those two sets produce are in `crates/memex-http/tests/fixtures/agui`, and `MEMEX_UPDATE_GOLDEN=1` rewrites them. `pnpm test` holds them to `@ag-ui/core`'s schemas. The scripted tests prove what the tools let happen and say nothing about what a model chooses to do. The live run calls an LLM, costs money and sends public corpus passages to the provider, so run it only when asked: spec 049's Result has the command.
- Chat is off unless `MEMEX_CHAT_MODEL` names a model, on the same provider as extraction, and then off per vault until an admin turns it on at `PUT /api/v1/vaults/{id}/chat/settings`. `MEMEX_CHAT_OUTPUT_TOKENS`, `MEMEX_CHAT_THREAD_TOKENS`, `MEMEX_CHAT_DAY_TOKENS`, `MEMEX_CHAT_TIMEOUT_S`, `MEMEX_CHAT_MODEL_CALLS` and `MEMEX_CHAT_MAX_RUNS` tune it.
- The binary: `memexvault migrate`, `memexvault bootstrap <name>`, `memexvault models fetch`, `memexvault serve` and `memexvault mcp`, with `MEMEX_DATABASE_URL` set.
- MCP is served at `/api/v1/mcp` by `serve`, and answers only the hosts in `MEMEX_MCP_ALLOWED_HOSTS`, by default this machine's. `memexvault mcp` is the same over stdio with `MEMEX_MCP_TOKEN`, and refuses to start beside `serve`. On Postgres: `cargo test -p memexvault --release --test mcp -- --ignored --test-threads=1 pg_`.
- The official inspector against the real binary, on an empty database of your own, through npx with no install: `MEMEX_DATABASE_URL=... MEMEX_MODELS_DIR=... PORT=... specs/029-mcp-server/inspector.sh`, after `cargo build -p memexvault`.
- The live agent run calls an LLM, costs money and sends passages to the provider, so run it only when asked: with a server running, set `MEMEX_MCP_URL`, `MEMEX_MCP_TOKEN`, `MEMEX_TEST_CORPUS`, `MEMEX_LLM_BASE_URL`, `MEMEX_LLM_MODEL`, `MEMEX_LLM_API_KEY` if it needs one, `MEMEX_EVAL_MAX_TOKENS` and `MEMEX_EVAL_APPROVED`, then `cargo test -p memex-mcp --test agent -- --ignored --nocapture --exact live_agent_answers_ten_golden_questions`. The scripted tests beside it say nothing about a model.
- The activity stream on Postgres, with no models: `cargo test -p memexvault --release --test activity -- --ignored --test-threads=1 pg_activity`. Its measurements run the real binary on a fresh database of your own, and need `MEMEX_MODELS_DIR` (with the OCR and `multilingual-e5-small` files) and `MEMEX_ACTIVITY_PORT`, by default 18038: `cargo test -p memexvault --release --test activity -- --ignored --nocapture --exact measure_ingest_with_a_client_that_never_reads` or `measure_fifty_clients`. `MEMEX_ACTIVITY_DOCS` sets the documents per ingest run, by default 400. The pass rules are at the top of that file.
- The stream's settings: `MEMEX_ACTIVITY_MAX_STREAMS` per principal (16), `MEMEX_ACTIVITY_BUFFER_FRAMES` (1024) and `MEMEX_ACTIVITY_BUFFER_BYTES` (1 MiB) per client, `MEMEX_ACTIVITY_POLL_MS` (200), `MEMEX_ACTIVITY_RECHECK_MS` (1000), `MEMEX_ACTIVITY_HEARTBEAT_S` (15) and `MEMEX_ACTIVITY_STALL_S` (30).

The console's commands run from `web/`: `pnpm install`, `pnpm build`,
`pnpm test`, `pnpm typecheck`, `pnpm lint`, `pnpm run licenses`, `pnpm run contrast`,
`pnpm e2e`, `pnpm api:gen`, `pnpm api:check` and `pnpm budget`.

The binary embeds `web/apps/console/dist` with `rust-embed`, so run `pnpm build` before `cargo build --release` to ship the console. Cargo notices a new build by itself, and a build without one serves a page that says so.
`pnpm dev` proxies `/api` to `MEMEX_API_URL`, by default `http://127.0.0.1:8080`, the default `MEMEX_HTTP_BIND`. The proxy keeps everything on one origin, so sign-in works in development, and production needs no CORS.
The browser session checks run against a real server: with `memexvault serve` running and its token from `bootstrap`,
`MEMEX_E2E_URL=http://127.0.0.1:<port> MEMEX_E2E_TOKEN=<token> pnpm --filter @memex/console e2e:session`. The URL must use `127.0.0.1`, as the cross-site test uses `localhost` as the other site.
The shell checks start and kill the server themselves, so nothing else may be serving that address. They use what `memexvault serve` needs, including `MEMEX_MODELS_DIR`, and the embedded console, so run `pnpm build` and `cargo build -p memexvault` first:
`MEMEX_E2E_BIN=$PWD/../target/debug/memexvault MEMEX_E2E_TOKEN=<token> MEMEX_HTTP_BIND=127.0.0.1:<port> MEMEX_DATABASE_URL=<url> pnpm --filter @memex/console e2e:shell`.
The page screens' checks do the same, with the same variables, on a database of their own: `pnpm --filter @memex/console e2e:wiki`. `MEMEX_E2E_EVIDENCE=<file>` also writes the hostile page's network log there.
The document viewer's checks do the same: `pnpm --filter @memex/console e2e:documents`. `MEMEX_E2E_RECORDING`, a recording Chromium can play, runs the player's checks, which transcribe it, so `MEMEX_MODELS_DIR` needs `whisper`. `MEMEX_E2E_PG_CONTAINER` and `MEMEX_DATA_DIR` run the bad cases, which change that database and data folder directly, so never point them at anything shared. `MEMEX_E2E_EVIDENCE=<file>` appends what was measured there.
The ingest screen's checks do the same, through a proxy of their own on the bind's port plus 100, which they cut, and with a stand-in for the LLM's server on this machine, so nothing calls a provider: `pnpm --filter @memex/console e2e:ingest`. `MEMEX_E2E_RECORDING`, an MP3 of speech, runs item 1, so `MEMEX_MODELS_DIR` needs `whisper`, and `MEMEX_E2E_PG_CONTAINER` runs item 4, which reads the log of that database, as no route shows who an event came from. `MEMEX_E2E_LOAD=1` runs the 5-minute load test at 50 events a second, and `MEMEX_E2E_LOAD_SECONDS` shortens it. `MEMEX_E2E_EVIDENCE=<file>` appends what was measured there.
The search screen's checks do the same: `pnpm --filter @memex/console e2e:search`. Its golden-set check needs `MEMEX_E2E_CORPUS`, spec 004's `corpus/passages.txt`, with the real models in `MEMEX_MODELS_DIR`, and is reported as skipped without it. `MEMEX_E2E_EVIDENCE=<file>` appends what was measured there.
The API types come from `web/packages/api/openapi.json`. After an API change, with a server running, from `web/`: `MEMEX_OPENAPI_URL=http://127.0.0.1:<port>/api/v1/openapi.json pnpm api:gen`, then `pnpm typecheck`. `pnpm api:check` with the same variable fails on drift from the server, or takes `--from <file>`. `pnpm typecheck` compares only the committed types with the committed document.
`pnpm budget`, after `pnpm build`, fails when the sign-in route's initial JavaScript passes 200 kB gzip.

Write down what was measured, not just that it was done.
Run `check` before committing spec changes, and name the spec in the commit
message (`spec #63: ...`). Older commits use `spec 063: ...`. Also run `wave-manager.py check` after changing waves or `Depends on` lines.

## License

Apache License 2.0. See [LICENSE](LICENSE). Some test fixtures hold Wikipedia text under CC BY-SA and aren't covered by it, as [NOTICE](NOTICE) lists. Keep any new third-party text in its own folder, with a README giving its source and license, and add it to `NOTICE`.
