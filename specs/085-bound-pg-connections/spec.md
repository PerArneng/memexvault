---
id: 085
name: bound-pg-connections
description: Put a limit on the connections PgStore opens, so busy servers wait instead of failing with too many clients
---

# 085 bound-pg-connections

**Depends on:** 032.

## Goal
One server never opens more Postgres connections than a set limit. When it's busy, requests wait briefly and then get a clear error, instead of the database refusing connections with `too many clients`.

## Background
`PgStore::take_conn` in `crates/memex-store-postgres/src/store.rs` opens a new connection whenever its pool is empty, and `pool_max`, 16 from `main.rs`, only limits how many idle ones it keeps. Nothing limits how many are open. Writers hold their own connections for advisory locks. The shared test server allows 64.

Reported on a machine running several workers' servers and test suites at once: spec 044 sampled 65-67 connections, spec 082's Postgres runs failed 27 and 7 tests almost all with `53300: too many clients`, and spec 041 saw 72 in use, a server that failed to start and vault creation getting 503s. Those are several processes together, so no single server has been measured against a limit yet. Spec 045's end-to-end setup and spec 041's checks now retry to get past this.

## Approach
- A limit on open connections per `PgStore`, with a setting and a default chosen from a measurement, that leaves room for the writers' lock connections, so a read burst can't stop a writer getting its own.
- Past the limit a caller waits up to a set time, then gets a backend error that says the pool is full. The HTTP status and message are decided here and recorded.
- Record how many connections each part of a server uses at idle and under load.
- `docs/architecture.md` says how to size this against the server's `max_connections`.

## Verification
1. With a limit of N and many concurrent readers, `pg_stat_activity` never shows more than N for that server, sampled through the run.
2. A burst of readers can't starve a writer: a vault's writer still opens and commits during the burst.
3. A caller past the limit gets the new error after the wait, and nothing hangs. A connection given back wakes a waiter.
4. Search and ingest latency with the default limit, on the bench in `crates/memexvault/tests/golden.rs`, against the same run without it, with the load recorded and the pass rule set before measuring.
5. A Postgres restart with callers waiting recovers with no restart of the server, and closed connections don't count against the limit.
6. Postgres tests pass, including a run with two other servers on the same database server.

## Out of scope
Pooling across processes, such as PgBouncer. Fixing `pg_bad_cases`' server-wide kill.

## Result
(Filled in when the spec moves to done or dropped: what was built, decisions, numbers, follow-ups.)
