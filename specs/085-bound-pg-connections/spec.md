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
Built on Linux x86_64 against a Postgres 17 container of this spec's own (`max_connections` 64), with nothing else on it. The `memexvault` crate couldn't build there, as `memex-pdf` turns on pdfium-render's macOS-only `core_graphics` feature and `scripts/fetch-pdfium.sh` has no Linux pin, so anything that needs the binary or that crate's tests is listed below as not run.

**What was built**
- `PgStore`'s pool counts every connection it opens, idle or taken, and never opens more than `PoolLimits::max_open`. A dropped connection gives its place back, through `Drop` on `Conn`, so a caller that fails and drops its connection frees it as surely as one that gives it back, and a closed connection stops counting as soon as it's dropped or found idle.
- Past the limit a caller waits up to `PoolLimits::wait`, then gets `PgError::Backend("the connection pool is full: all N connections are in use, and none came free within W ms")`. Waiters are served in the order they came. The first version woke one waiter at a time, and the 48-thread sweep showed a thread giving a connection back and taking it again before the woken waiter could, so waiters could starve until they timed out. Now each waiter holds a ticket and only the oldest may take what comes free.
- **HTTP status, decided here:** the error is a store backend error, so it becomes `ServerError::Unavailable` and the API returns 503 with code `unavailable` and the usual "a backend is unavailable, so try again". The detail, including "pool is full", goes to the log. No new code or status, so nothing changes in the API or the console.
- Writers' lock connections stay outside the pool, one per vault this server writes to, so a burst of readers can't stop a writer opening its vault or committing.
- Settings: `MEMEX_PG_MAX_CONNECTIONS` (32) and `MEMEX_PG_WAIT_MS` (5000), both refusing 0. `serve`, `mcp`, `migrate` and `bootstrap` keep up to 16 idle. `PgStore::connect(url, idle)` keeps the defaults, so existing tests and tools are unchanged.
- `check_min_seq` in search held a snapshot while the vector index took a connection of its own. 32 such searches at once could each hold one and wait for a second until they all failed. It now gives the snapshot back first, and takes a new one only when an index's watermark is behind `min_seq`. A later snapshot only sees more of the log, so the answer is the same.
- `docs/architecture.md` says how to size the limit against `max_connections`. AGENTS.md has the test command and the settings.

**Verification** (`crates/memex-store-postgres/tests/pool.rs`, `--release --test-threads=1`)
1. Pass. Limit 6, 48 readers × 20 queries of `pg_sleep(0.01)`: 960 queries in 1.69 s, and `pg_stat_activity` for that store's application name peaked at 6 over 273-282 samples taken every 5 ms. The store's own count also peaked at 6. Same result in 3 runs.
2. Pass. Limit 3, 200 ms wait, 24 readers holding connections for 50 ms in a loop: with the pool full, the writer opened a new vault in 91-137 ms, which includes creating its schema, and made 20 commits (median 1.45-1.95 ms, max 2.99-3.40 ms). Meanwhile the readers got 12-21 connections and 12-19 pool-full errors. Afterwards that store had 4 connections: 3 pooled and 1 holding the advisory lock.
3. Pass. Limit 2, 300 ms wait: a third caller was refused after 300.03-300.09 ms with the error above. With a 10 s wait, a connection given back at 200 ms reached the waiter 79-269 µs after the give, and a connection dropped instead at 200 ms let the waiter open a new one at 205-206 ms. Fairness: limit 1, a thread taking and giving back as fast as it could (70-73 turns), and 50 takes beside it waited 2.2 ms at the median and 2.6 ms at most.
4. **Not run as written.** The golden bench needs spec 004's corpus and spec 005's models, and neither is on this machine. The ingest half needs the binary. As a substitute, before measuring, the rule was: at a load within the limit, the limit costs nothing beyond noise, and above it, it costs throughput but keeps the count bounded. Measured: each of 16 or 48 threads read `count(*)` from `pg_class` 200 times, timed from asking for a connection. Runs 2 and 3, after the fairness fix:

   | threads | limit | wall | p50 | p95 | p99 | max |
   |---|---|---|---|---|---|---|
   | 16 | 8 | 302-320 ms | 1.45-1.53 ms | 1.75-1.94 ms | 2.1-3.1 ms | 11 ms |
   | 16 | 16 | 252-265 ms | 1.16-1.21 ms | 1.44-1.65 ms | 1.7-1.9 ms | 22-23 ms |
   | 16 | 32 (default) | 240-262 ms | 1.12-1.21 ms | 1.38-1.52 ms | 1.5-2.3 ms | 19-23 ms |
   | 16 | none | 242-264 ms | 1.10-1.21 ms | 1.40-1.47 ms | 1.6-1.8 ms | 23-25 ms |
   | 48 | 8 | 1.19 s | 5.80 ms | 6.94 ms | 7.7-7.8 ms | 13 ms |
   | 48 | 16 | 1.11-1.14 s | 5.41-5.56 ms | 6.3-6.7 ms | 8.0-8.2 ms | 23 ms |
   | 48 | 32 (default) | 1.04-1.08 s | 5.04 ms | 6.3-6.8 ms | 8.2-11.3 ms | 45-52 ms |
   | 48 | none | 0.86-0.91 s | 3.99-4.24 ms | 4.9-5.2 ms | 7.6-8.4 ms | 63-69 ms |

   Within the limit (16 threads), the default matches no limit. With 48 threads, 32 connections took 17-20% longer in all than 48, and limit 8 only 10-15% longer than 32. On this machine more connections buy little throughput, so **32 was chosen as the default**. It leaves room for a second server, writers and tools under Postgres's default of 100, and stays at half of the test server's 64.
5. Pass. Limit 4, with 2 idle connections, 4 held and 3 waiters (20 s wait): `podman restart` of this spec's own container took 0.40-0.73 s. The 4 held connections failed and their callers dropped them. All 3 waiters then got working connections, 0.61-0.94 s after they started waiting. Afterwards the pool counted 3 open, all from after the restart. Then 4 taken at once gave 4 working connections, `pg_stat_activity` showed exactly 4, and none from before the restart counted.
6. Partly run. Three processes of the pool tests at once on one database server, each with a store of its own, all passed (4 tests each), and the server peaked at 22 client connections over 150 samples. A first attempt also ran the no-limit half of the latency sweep, 48 connections per process, and those three processes hit `53300: too many clients`, the failure this spec is about. **Not run:** the `memexvault` crate's Postgres suites (`pg_` in `pg_tests`, vector, mcp, chat and the rest) and a run beside two real servers, because that crate doesn't build on Linux.

**Connections per part, from the code, as no server could run here:** the pool serves every read and short write (snapshots, cache marks, the registry, the vector index, chat, migrate), and is at most `MEMEX_PG_MAX_CONNECTIONS`. At idle it holds at least the 1 connection opened at startup, and at most 16. Each writer holds 1 more for as long as its vault is open. This wasn't measured on a real server at idle or under load.

**Other checks:** `cargo clippy --all-targets -- -D warnings`, `cargo test` (875 passed, 0 failed, 10 ignored) and `cargo fmt --check` pass on every crate except `memex-pdf` and `memexvault`. The full-workspace `cargo build`, `cargo test` and `cargo clippy` stop at pdfium-render on Linux, before any of this spec's code is compiled.

**Follow-ups**
- On macOS, run the `memexvault` Postgres suites, item 4 on the golden bench, and item 6 beside two real servers, and record each part's connections at idle and under ingest from `pg_stat_activity`.
- Look for other places that hold one pooled connection while taking a second, such as a snapshot held across a vector index call. At the limit, such callers can only end in the pool-full error after the wait, and not in a hang.
- A Linux pdfium pin and a feature set that builds there, so the workspace builds on Linux.
