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
- The keyword index harness needs Postgres and the corpus from spec 004: set `MEMEX_TEST_DATABASE_URL`, `MEMEX_TEST_DATA` and `MEMEX_TEST_CORPUS`, then
  `cargo test -p memexvault --release --test lexical_harness -- --ignored --nocapture --exact lexical_bench_200k` or `lexical_kill_200_times`.
- The vector index needs Postgres: `cargo test -p memexvault --release --test vector -- --ignored --test-threads=1 pg_`. `real_model_matches_the_spike_and_is_fast_enough` in that file also needs `MEMEX_TEST_E5` (`model.onnx` and `tokenizer.json`), and `MEMEX_TEST_CORPUS` and `MEMEX_TEST_VECTORS` from spec 004. The harness uses the same variables: `cargo test -p memexvault --release --test vector_harness -- --ignored --nocapture --exact vector_bench_200k` or `vector_kill_200_times`.
- Search needs Postgres, the same data, and `MEMEX_TEST_DATA`, `MEMEX_TEST_E5` and `MEMEX_TEST_RERANKER` (spec 005's `mmarco` folder): `cargo test -p memexvault --release --test golden -- --ignored --nocapture --exact golden_recall` or `latency_50k_200k`, on the golden set in `crates/memexvault/tests/fixtures/golden/`.
- OCR tests are ignored unless `MEMEX_TEST_MODELS` names a folder of model files: `cargo test -p memexvault --release --test pdf_pipeline -- --ignored --test-threads=1`.
- The binary: `memexvault migrate`, `memexvault bootstrap <name>`, `memexvault models fetch` and `memexvault serve`, with `MEMEX_DATABASE_URL` set.

The console's commands run from `web/`: `pnpm install`, `pnpm build`,
`pnpm test`, `pnpm typecheck`, `pnpm lint`, `pnpm run licenses`, `pnpm run contrast`
and `pnpm e2e`. Write down what was measured, not just that it was done.
Run `check` before committing spec changes, and name the spec in the commit
message (`spec #63: ...`). Older commits use `spec 063: ...`. Also run `wave-manager.py check` after changing waves or `Depends on` lines.

## License

Apache License 2.0. See [LICENSE](LICENSE).
