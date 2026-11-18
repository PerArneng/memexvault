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
Built and measured on Linux x86_64 (16 cores, shared with five other workers, load average 2 to 55 during the runs) against a Postgres container of this spec's own with `max_connections` 64. The first round ran before spec 091 fixed pdfium on Linux, so it tested only the store crate. The second round, after merging `main` with specs 091 and 086, ran everything below.

**What was built**
- `PgStore`'s pool counts every connection it opens, idle or taken, and never opens more than `PoolLimits::max_open`. A dropped connection gives its place back, through `Drop` on `Conn`, so a caller that fails and drops its connection frees it as surely as one that gives it back, and a closed connection stops counting as soon as it's dropped or found idle.
- Past the limit a caller waits up to `PoolLimits::wait`, then gets `PgError::Backend("the connection pool is full: all N connections are in use, and none came free within W ms")`. Waiters are served in the order they came. The first version woke one waiter at a time, and the 48-thread sweep showed a thread giving a connection back and taking it again before the woken waiter could, so waiters could starve until they timed out. Now each waiter holds a ticket and only the oldest may take what comes free.
- **HTTP status, decided here:** the error is a store backend error, so it becomes `ServerError::Unavailable` and the API returns 503 with code `unavailable` and the usual "a backend is unavailable, so try again". The detail, including "pool is full", goes to the log. No new code or status, so nothing changes in the API or the console.
- Writers' lock connections stay outside the pool, so a burst of readers can't stop a writer opening its vault or committing. A server takes the lease of every vault it knows and holds it while it runs, so **a server uses its pool limit plus one connection per vault**. Item 6 showed what that costs, below. The user decided to keep it out of this spec, and spec 093 is the follow-up.
- Settings: `MEMEX_PG_MAX_CONNECTIONS` (32) and `MEMEX_PG_WAIT_MS` (5000), both refusing 0. `serve`, `mcp`, `migrate` and `bootstrap` keep up to 16 idle. `PgStore::connect(url, idle)` keeps the defaults, so existing tests and tools are unchanged.
- `check_min_seq` in search held a snapshot while the vector index took a connection of its own. 32 such searches at once could each hold one and wait for a second until they all failed. It now gives the snapshot back first, and takes a new one only when an index's watermark is behind `min_seq`. A later snapshot only sees more of the log, so the answer is the same.
- `docs/architecture.md` says how to size servers against `max_connections`: each server's limit plus its vault count, plus reserved connections and tools. AGENTS.md has the test command and the settings.

**Verification.** The store-level checks are in `crates/memex-store-postgres/tests/pool.rs` (`--release --test-threads=1`). Server-level runs used `memexvault serve` from a release build, with a scripted load: one vault, 40 or 200 markdown uploads from 8 threads, then 30 s of hybrid searches from 8, 16 or 48 threads.
1. **Pass.** Limit 6, 48 readers × 20 queries of `pg_sleep(0.01)`: 960 queries in 1.69-2.86 s, and `pg_stat_activity` for that store's application name peaked at 6 over 173-282 samples taken every 5 ms. The store's own count also peaked at 6. Same result in 6 runs, including 2 beside two loaded servers. On real servers with a limit of 10, under load, each peaked at 11: 10 pooled and 1 writer.
2. **Pass.** Limit 3, 200 ms wait, 24 readers holding connections for 50 ms in a loop: with the pool full, the writer opened a new vault in 91-574 ms, which includes creating its schema (the slow ones beside the loaded servers), and made 20 commits (median 1.3-13 ms, max 3-29 ms). Meanwhile the readers got 12-55 connections and 12-61 pool-full errors. Afterwards that store had 4 connections: 3 pooled and 1 holding the advisory lock.
3. **Pass.** Limit 2, 300 ms wait: a third caller was refused after 300.03-301.8 ms with the error above. With a 10 s wait, a connection given back at 200 ms reached the waiter 79-269 µs after the give, and a connection dropped instead at 200 ms let the waiter open a new one at 205-206 ms. Fairness: limit 1, a thread taking and giving back as fast as it could (10-73 turns), and 50 takes beside it waited at most 1.7-3.7 ms.
4. **Fails its rule above the limit, passes within it.** The golden bench needs spec 004's corpus and spec 005's models, and neither is on this machine, so a real server with a scripted load stood in for it. The rule, set before measuring: with the default limit, upload p50 and search p50 and p95 within 15% of no limit (`MEMEX_PG_MAX_CONNECTIONS=100000`), over 2 alternating runs each, with no pool-full errors. Each run used a fresh database, a warm-up round to build the vector index, then 200 uploads and 30 s of searches.

   | searchers | limit | load avg | searches in 30 s | search p50 | search p95 | upload p50 | peak connections | pool-full errors |
   |---|---|---|---|---|---|---|---|---|
   | 48 | 32 (default) | 12.5-25.5 | 9,483-13,121 | 106-147 ms | 155-211 ms | 57-100 ms | 33 | 0 |
   | 48 | none | 18.5-25.6 | 11,295-16,011 | 86-123 ms | 123-175 ms | 56-115 ms | 55-56 | 0 |
   | 16 | 32 (default) | 24.5-25.8 | 12,323-12,969 | 39.8-41.8 ms | 57.9-59.4 ms | 107-113 ms | 23 | 0 |
   | 16 | none | 24.4-28.1 | 12,351-12,450 | 41.1-41.5 ms | 60.6-61.6 ms | 114-126 ms | 23 | 0 |

   With 48 searchers, more than the limit, the default's search p50 was 19-26% above no limit and its p95 15-34% above, in 2 runs and in an unchanged rerun once the load average was checked. So it **fails** the 15% rule there. Uploads passed, and no request failed. With no limit the same server held 55-56 connections, close to all 64. With 16 searchers, within the limit, the two were the same. The store-level sweep agrees: within the limit it costs nothing, and above it, 48 threads on 32 connections take 16-20% longer than on 48 connections:

   | threads | limit | wall | p50 | p95 |
   |---|---|---|---|---|
   | 16 | 8 / 16 / 32 / none | 302-647 / 252-547 / 240-562 / 242-546 ms | 1.45-3.06 / 1.16-2.49 / 1.12-2.59 / 1.10-2.53 ms | 1.75-4.30 / 1.44-3.85 / 1.38-3.94 / 1.40-3.78 ms |
   | 48 | 8 / 16 / 32 / none | 1.19-2.39 / 1.11-2.42 / 1.04-2.29 / 0.86-1.97 s | 5.80-11.7 / 5.41-11.7 / 5.04-10.6 / 3.99-9.16 ms | 6.94-14.7 / 6.3-16.4 / 6.3-16.6 / 4.9-12.4 ms |

   (Each range spans the runs, with the slow ends from the post-merge run at a load average of about 40.) **Item 4 failed against its pre-set 15% rule,** measured with a scripted load on a real server instead of the golden data, at load average 12-25. With 48 searchers, search p50 was 19-26% slower and p95 15-34% slower than with no limit. With 16 searchers the two were the same. Peak connections were 33 against 55-56 with no limit. **On 2026-11-18 the user accepted queueing above the limit as the intended trade-off**, and the default of 32 was kept. Raising it to pass at 48 searchers would give back most of the protection.
5. **Pass.** Store level: limit 4, with 2 idle connections, 4 held and 3 waiters (20 s wait), `podman restart` of this spec's own container took 0.40-0.73 s. The 4 held connections failed and their callers dropped them. All 3 waiters then got working connections, 0.61-0.94 s after they started waiting. Afterwards the pool counted 3 open, all from after the restart. Then 4 taken at once gave 4 working connections, `pg_stat_activity` showed exactly 4, and none from before the restart counted. Server level: with `serve` taking a search every 250 ms, a 0.8 s Postgres restart cost 3 of 60 searches a 503, searches were back 0.38 s after it, and the server kept running with 5 connections. Its writer lease didn't come back by itself, which is spec 089's area.
6. **Pass with one vault per server. Two servers that made a new vault each round filled the database server.**
   - Alone, the `memexvault` Postgres suites pass: 40 tests in `activity`, `audit`, `cache_tiers`, `chat`, `entities`, `extraction`, `mcp`, `meeting`, `pages`, `review` and `vector`. In the unit tests, 10 `pg_` tests pass, including `pg_kill_postgres_100_times` (through a `docker` shim that runs `podman`, on this spec's own container), `pg_kill_memexvault_1000_times` and `pg_kill_jobs_200_times`. Five fail without involving the pool:
     - `pg_bad_cases` makes its "nothing listens there" URL by replacing the port `55432`, so on any other port it connects. It also kills advisory locks across the whole server, so it wasn't run on the shared one.
     - `pg_child`, `pg_job_child` and `pg_job_child_long` are helpers the kill harnesses start with their own variables.
     - `pg_full_disk_…` needs a Postgres on a small disk.
   - **First run beside two servers.** Two `serve` processes with `MEMEX_PG_MAX_CONNECTIONS=10`, each loaded by a script that made a new vault every round, ran beside the suites. Their pools stayed at 10, but writers grew with the vaults: 31 and 30 connections, 21 and 20 of them writers. The database server reached 63 of 64, and 6 tests in `pages` and `vector` failed with `53300: too many clients`.
   - **Rerun with one vault per server.** Both servers peaked at 11 (10 pooled and 1 writer), and the database server peaked at 39 over 787 samples, with no `too many clients`. All 40 integration tests and the 4 pool tests passed, at load average 21 to 55. The servers served 1,120 uploads each, and 129,674 and 127,926 searches with status 200. They also returned 1,018 and 837 with 503, all logged as "the vector index is still being built" and all in the first round.

**Connections per part, measured on a real server** (one vault, `pg_stat_activity` by application name, writers found through `pg_locks`):
- At idle after startup, the pool held 6-8 connections, kept by the startup work, the activity readers and the job workers, up to the idle cap of `min(limit, 16)`. A server with no vault held 1.
- Under load it held its limit: 10 of 10, or 32 of 32 with the default. Each vault's writer held 1 more.
- With no limit, 48 searchers drove one server to 55-56.

**Other checks** (after merging `main` at spec 087, `b3728c4`): `cargo build --workspace`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check` pass. `cargo test --workspace` gives 1001 passed, 0 failed and 118 ignored. The 6 pool tests pass, and so do the `memexvault` integration `pg_` suites, 43 tests, at load average 7 to 17. `gh-spec-manager.py check` passes.

**Follow-ups**
- Spec 093, bound-writer-connections: cap writer connections or release idle leases, so a server's connections don't grow with its vault count.
- The golden bench (item 4 as written) on a machine with spec 004's and 005's data.
- `pg_bad_cases` assumes port 55432 for its bad URL.
- Look for other places that hold one pooled connection while taking a second. At the limit, such callers can only end in the pool-full error after the wait, and not in a hang.
