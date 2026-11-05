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
7. Exact lookup batched for 50 ids has p95 ≤2 ms at 1M rows on the same machine, or the revised target from spec 006.

## Out of scope
Blobs (spec 011), backup (spec 030), replication and failover, a second SQL database, and several active servers on one vault.

## Result
Not started.
