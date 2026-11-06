---
id: 032
name: sql-store-backend
description: Implement the store on PostgreSQL with real tables, migrations and group commit
---

# 032 sql-store-backend

## Goal
A durable `Store` on PostgreSQL with real relational tables and one schema per vault, behaving exactly like the fake and
surviving crashes of both memexvault and Postgres. It replaces the dropped spec 010 and comes right after 009 despite its id.

## Background
**Depends on:** 009, and results from 004 and 006. Design: `docs/architecture.md`, sections PostgreSQL, Backends, Consistency.

The user chose Postgres as the real database and allows the pgvector extension.

## Approach
- `memex-store-postgres` with SQL only here, plus the trait implementation in the binary crate as the orphan rule requires.
- Versioned migrations embedded in the binary, an explicit `migrate` command, and one schema per vault. Tables for the object
  kinds that exist so far, and later specs add theirs as new migrations.
- A group-commit writer: one writer per vault, batched into one SQL transaction, acknowledged only after durable commit. The
  batching decision is a pure function.
- The writer lease is a session advisory lock. Snapshots use repeatable read, and batched lookups are one query each.
- Startup checks: schema version, `fsync` and `synchronous_commit` on, and pgvector present if configured.
- Connection pool with limits and reconnect. The URL comes through `Settings` and is redacted from output.
- Tests reach a server named by `MEMEX_TEST_DATABASE_URL`, normally from `compose.yaml`. A run without it reports the suite as
  not run, never as passed.

## Verification
1. The unchanged `Store` contract suite from spec 009 passes on `MemStore` and on Postgres.
2. Kill harness: 1,000 cycles killing memexvault and 100 killing Postgres, during commits: 0 lost acknowledged events, 0 partial
   transactions and 0 reopen failures.
3. Replay of 100k events through Postgres has the same dump hash as through `MemStore`.
4. ≥1,000 events/s sustained for 60 s, or the revised target recorded by spec 006.
5. `git diff` for this spec adds no SQL or Postgres type to a pure crate, and `cargo tree -p memex-core` lists no Postgres client.
6. Bad cases: a second writer on one vault is refused; a lost connection stops the writer until it regains the lease; a full disk
   returns an error and data stays readable; an unknown schema version is refused; `synchronous_commit=off` is refused at startup;
   the URL's password appears in no log line.
7. Exact lookup batched for 50 ids has p95 ≤2 ms at 200k rows on the same machine, with a projection to 1M, or the revised target from spec 006.

## Out of scope
Blobs (spec 011), backup (spec 030), replication and failover, a second SQL database, and several active servers on one vault.

## Result
Built `memex-store-postgres`: real tables in one schema per vault, versioned migrations embedded in the binary with a `migrate` command, a writer lease on a session advisory lock, repeatable-read snapshots on a pooled connection, batched lookups, startup checks and a statement cache per connection. `PostgresStore` in the binary implements the core trait. The group-commit writer is `writer.rs`, a thread per vault with the pure `plan_batch` decision, and `Engine::apply_batch` reduces each draft on top of the ones before it, through the pure `Pending` overlay. All runs below are from one frozen test binary against Postgres in Docker, TCP, on a clean database. Output is in `results.txt`.

**Verification.**
1. The unchanged contract suite passes on `MemStore` and on Postgres (`pg_contract`), and so does the registry's.
2. Kill harness, with 4 producers through the group writer: **1,000 kills of memexvault: 0 lost, 0 partial, 0 gaps, 0 reopen failures, 64,976 events acknowledged. 100 kills of Postgres: all zero as well, 7,224 acknowledged, recovery p95 0.6 s.**
3. A 100,000-event random run replays through Postgres with the same dump hash as `MemStore`: 34,130 applied, 65,870 refused by the reducer, hash `ab882472…`, 129 s.
4. Group commit, small events with one row each:

| Producers | Duration | Events/s | Ack p50 | Ack p95 |
|---|---|---|---|---|
| 1 | 20 s | 239 | 3.6 ms | 7.9 ms |
| 16 | 60 s | **2,121** | 6.8 ms | 15.9 ms |
| 64 | 20 s | 7,693 | 8.2 ms | 15.4 ms |

≥1,000 events/s sustained for 60 s: pass. One producer waiting for each ack is limited by commit time, as spec 006 found. The first measurement was only 627 and 786 events/s, because the reducer read once per event and every statement was prepared again each time. I fixed both, with `Pending::prefetch` and the statement cache, and a test now proves a batch's reads don't grow with its size.

5. `cargo tree -p memex-core` lists no Postgres client, and no SQL or Postgres type is in a pure crate.
6. Bad cases, in `pg_bad_cases`: a second writer gets `WriterActive`. Terminating the writer's connection gives `LeaseLost` until it opens again, which works. A text value Postgres can't store fails with no trace. An unknown schema version is refused by both `check_server` and `migrate`. `synchronous_commit=off` is refused at startup. The URL's password is in no error or debug output. Full disk, in `pg_full_disk_…` on a throwaway container with a 24 MB tablespace: after 5 commits the next returns `53100: could not extend file…: No space left on device`, and earlier data stays readable.
7. Batched lookup of 50 ids: p95 **1.66 ms** at 200k rows and 1.24 ms at 50k, single lookup p95 1.09 ms, target ≤2 ms: pass. Growth from 50k to 200k is about 30% for the batch, which projects to about 2.2 ms at 1M if it continues to grow that way, slightly over target. Spec 033 measures it. Loading 200k rows took 5 s, down from 216 s before the fixes.

**Decisions and notes.**
- Newer kinds store the row as JSONB beside indexed columns. Older kinds have columns for every field.
- A new kind needs a vault migration and an arm in `upsert_json_row`.
- A test run without `MEMEX_TEST_DATABASE_URL` shows these tests as ignored, and run with `--ignored` without it they fail with "not run", so they can't pass by accident.
- TLS isn't done, tracked in spec 059. WAL filling a disk isn't tested, only table data: Postgres panics by design when WAL can't be written, and recovers once space is freed.
- This whole run relies on Docker's flush, which spec 006 showed doesn't reach the drive. It proves process-crash safety, not power-loss safety.
