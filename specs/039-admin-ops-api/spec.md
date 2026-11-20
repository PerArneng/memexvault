---
id: 039
name: admin-ops-api
description: Expose operations, settings, cost and system status over HTTP for the admin console
---

# 039 admin-ops-api

## Goal
Everything an operator does on a running server can be done over HTTP, with progress and reports. Commands that have to run before the server is up stay on the command line.

## Background
**Depends on:** 021, 024, 030. Design: `docs/architecture.md`, sections API conventions, Backends.

## Approach
- Reindex, generation rollback, reconcile, export, import, orphan sweep and compaction as admin-scoped job routes with report retrieval.
  `migrate` and `models fetch` stay CLI-only, because they run before the server can serve anything.
- Effective settings, read-only, with secrets redacted. Active backends, models and their status, and thresholds.
- Status: Postgres and LLM reachability, index lag, queue depth, sizes, memory, and cache stats with a clear-cache action.
- An LLM usage report by agent, model, vault and day, built from Derivation token counts. It shows cost only where a price is configured, and it never
  guesses one.
- MCP connection info: endpoint, transports, protocol version and tool list.

## Verification
1. Each operation runs through its route, reports progress in the jobs API, and gives the same result as the command.
2. A secrets test: a marker set as the database password, LLM key and transcriber key appears in no settings, status, log or error response.
3. The token report matches a sum over Derivations on a fixture vault, and shows no cost when no price is set.
4. Every route is refused without admin scope, shown by a scope matrix.
5. With Postgres or the LLM down, status reports it correctly and promptly, within 2 s.
6. Bad cases: starting a second reindex of the same kind is refused with a clear error, and restoring over an existing vault needs the explicit flag.

## Out of scope
Runtime editing of settings, external monitoring integrations, and billing.

## Result
Done. Measured on 2026-11-20 on Linux x86_64, against a pgvector 0.8.7 / Postgres 18 container of this worktree's own (podman,
`127.0.0.1:55439`), as no Postgres was running on 55432. No LLM or provider was called: the LLM and the transcription API pointed at
`127.0.0.1:1`.

**Decisions** (asked of the coordinator, who took the recommendations, and recorded in `docs/architecture.md`, Operations):
- With Postgres down no token can be checked, so `GET /api/v1/system/status`, and only it, accepts a server admin's credential this process
  checked in the last 5 minutes, held in memory as a SHA-256, when the registry couldn't be reached. A refusal is never overridden.
- Imports are server-level operations held in the server's memory at `/api/v1/imports/{id}`, gone after a restart, since jobs live in a vault's
  schema and the vault doesn't exist yet. Every other operation is a durable `operation` job.
- Prices come from `MEMEX_LLM_PRICES` (`model=input:output[:thinking]`, US dollars per million tokens). A model not listed shows no cost.

**What was built.**
- `memex_types::OPERATION_JOB` and `OperationInput` (`reindex`, `rollback` of `keyword` or `vector`, `reconcile`, `export`, `sweep`,
  `compact`). `JobRow` gained `progress` (step, of, label) and `report`, both skipped when empty, so older rows and dump hashes don't change. A
  new attempt clears them. `JobDto` shows both, so the jobs API carries progress and reports for any job that writes them.
- `memex-core/src/server/admin.rs`: `start_operation` (409 naming the open job for a second of the same kind, or a rollback during a reindex of
  that index), `run_operation_job` (writes each step, then the report; reindex waits for the projector's build to finish; rollback forgets the
  projector's progress so it catches up from the old generation), the usage report as a pure function over Derivations, `clear_caches`,
  `check_restore` (reads only the archive's header), `RecentAdmins`, and the `ArchiveStore` and `OperationEdge` traits.
  `memexvault/src/admin_edge.rs` keeps archives in `<data>/archives`, written beside their name and renamed when whole, and reads memory from
  `/proc/self/statm`. One `operation` worker per server.
- Routes (`memex-http/src/routes/admin.rs`, merged in by the composition root like chat's): `POST|GET /vaults/{id}/operations`,
  `GET /vaults/{id}/operations/{jid}` and `.../archive`, `POST /imports` (`new_id`, `replace`, `name`), `GET /imports/{id}`,
  `GET /system/settings`, `/system/status`, `/system/usage` (`from`, `to` as UTC days) and `/system/mcp`, and `POST /caches/clear`.
  `Settings::effective` gives the settings with the database URL reduced to scheme, host and database, and keys and tokens shown as `<set>`.
  `LlmSettings`' and `TranscribeApi`'s `Debug` now redact a password in a base URL too, which they printed before.
- Spec 030's synchronous reconcile, reindex and sweep routes are unchanged.

**Verification.**
1. *Operations through their routes.* `cargo test -p memex-http --test admin`, in memory: every operation ran through its route and the
   jobs API showed the same progress and report as the operation's route; the reconcile report's tables hash equals the command's
   `reconcile_store` replayed `--at` the report's head; the export archive verifies, its dump hash equals the vault's at its head, and an import
   under a new id restores to that hash. On the real binary, `specs/039-admin-ops-api/tools/admin_live.py` (debug build, one uploaded
   document): reindex vector 0.48 s, keyword reindex then rollback 0.11 s, reconcile `deep` 0.21 s, sweep 0.11 s, compact 0.21 s, export
   0.16 s (40,203 bytes), each seen as `queued`, `running` with step 1/2 and its label, then `done` 2/2 in the jobs API. `memexvault reconcile
   --at <head>` printed the report's tables hash; `memexvault restore --new-id` of the downloaded archive and an HTTP import with `new_id`
   both reconciled to the export's dump hash. Passes.
2. *Secrets.* The live run used a Postgres role whose password, the LLM key and the transcription key were all the marker. 85 response bodies
   (settings, status up and down, every operation, imports, errors) and 167,446 bytes of server and command output held it 0 times. In memory,
   the same with the marker also in the LLM base URL's user info, the URL's query and the MCP token, over settings, status, MCP and five error
   responses. Passes.
3. *Token report.* Five Derivations over two vaults, two agents, two models and two days: each row's derivations and input, output and
   thinking tokens equal sums computed in the test over the rows, `unpriced_tokens` equals their whole total, and no row has a cost. With
   `m1=2:8:8`, m1's rows cost exactly their tokens times the price and m2's still show none. Passes.
4. *Scope matrix.* All 13 cases (the 11 new routes, two on another vault) for read, propose and admin in a vault, no grants, and no token: vault routes 403 below admin,
   404 without a grant to the vault and for another vault, 2xx for its admin; server routes 403 for all four, 401 without a token, and 200 for
   a server admin. Refused calls left the registry's fingerprint unchanged and refused imports wrote nothing. Spec 037's walk of the contract
   checks every new route refuses no credentials and takes a cookie. Passes.
5. *Down, within 2 s.* Live, Postgres behind a proxy: cut (connections refused), status answered 200 in 1.3, 2.3 and 1.9 ms with
   `postgres.reachable` false and `caller_from_memory` true, and `/system/settings` answered 503; stalled (connections open, nothing answering),
   1,604.6, 1,604.5 and 1,604.4 ms; after each, Postgres read as reachable again within 0.06 s. Up, 7.7 ms. The LLM: refused 2-4 ms, a black
   hole (`10.255.255.1:9`) 903-905 ms, off reported as not configured, a listening port as reachable. Passes.
6. *Bad cases.* A second keyword reindex while the first was queued: 409, "a keyword reindex is already queued in this vault, as job ….
   Wait for it to finish, or cancel it first", live and in memory, where a rollback of that index was refused too and a vector reindex wasn't.
   An import over the existing vault without `replace`: 409 at once, "vault … already exists, and a restore overwrites a vault only when told
   to replace it", with nothing left in the archive folder; with `replace` it restored over it. Passes.

Also: `cargo build --workspace`, `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo deny check licenses`
pass, and `cargo test --workspace` passes 1,037 tests with 126 ignored (with `MEMEX_PDFIUM_LIB` set, as this worktree has no `vendor/`).
On that Postgres, `cargo test -p memexvault --release -- --ignored --test-threads=1 pg_` passed 57, spec 030's `ops` and the job lease and
kill tests among them (with `MEMEX_TEST_CONTAINER` set to the container), and `memex-store-postgres`'s ignored tests passed 10. Five failed for
reasons outside this spec: `pg_child`, `pg_job_child` and `pg_job_child_long` are helpers the kill tests start with variables of their own, so
they fail run alone; `pg_bad_cases` makes its unreachable URL by replacing port 55432, which here left a working one; and
`pg_full_disk_returns_an_error_and_data_stays_readable` needs a Postgres on a small disk.
In `web/`: `pnpm api:gen` from the new document (additions only: ten paths, eleven schemas, two optional `JobDto` fields), then
`pnpm api:check` against the server, `pnpm typecheck`, `pnpm test` (762) and `pnpm lint` pass.

**Left as found.** `memexvault restore` beside a running `serve` can lose the new vault's writer lease to it, as `serve` takes the lease of
every new vault, so the live script restores after stopping the server. Review and audit calls write no Derivation, so the usage report
doesn't count them. The activity stream shows an operation job's state, as kind `other`, but not its progress label.
