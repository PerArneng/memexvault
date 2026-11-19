---
id: 093
name: bound-writer-connections
description: "Cap the writer lock connections a server holds, or release idle leases, so its connections don't grow with its vault count"
---

# 093 bound-writer-connections

**Depends on:** 085, 089.

## Goal
One server's Postgres connections stay under a set number however many vaults it has. Today each vault costs one connection for as long as the server runs. Writes to a vault keep working at any vault count, and a write that has to wait for a writer connection waits briefly and then gets a clear error, as reads do since spec 085.

## Background
Spec 085 limited the pool that serves reads and short writes, and left each writer's lock connection outside that limit. `Server::open_vaults` takes the writer lease of every vault the server knows at startup, and `Server::lease` takes one for each new vault. A lease is a session advisory lock on a connection of its own, held until the server stops. So a server's connections are its pool limit plus its vault count.

Spec 085's item 6 ran two servers, each with a pool limit of 10, on a Postgres with `max_connections` 64, beside the Postgres suites. The load made a new vault each round. The servers reached 31 and 30 connections, 21 and 20 of them writers. The database server reached 63 of 64, and 6 tests in `pages` and `vector` failed with `53300: too many clients`. With one vault per server, the same run peaked at 11 per server and passed.

Spec 089 changes how a lost lease is noticed and taken back, so this builds on it.

## Approach
- Measure first: connections per server, writer connections, and the cost of taking a lease (lock, schema check, `purge_incomplete`), at 1, 10 and 100 vaults.
- Choose between, and record why:
  - **Release idle leases.** A vault with no write for a set time gives back its lease and connection, and the next write takes the lease again. This needs to show that a lease taken again fences correctly, and that a second server can't slip in while it's released, or that it's acceptable when it does.
  - **Cap writers.** A set number of writer connections, shared by taking and releasing leases as writes need them, with a wait and a clear error past it.
  - **Share one connection between vaults.** One connection could hold several vaults' advisory locks. Losing it loses all of them at once, so that has to be acceptable.
- A setting with a default chosen from the measurements, and `docs/architecture.md`'s sizing rule changed to match.

## Verification
1. A server with 200 vaults never holds more than its limit, pool and writers together, sampled from `pg_stat_activity` through a run that writes to all of them.
2. Writes to every vault succeed in that run, and a write past the writer limit waits, then gets the error, and nothing hangs.
3. The fencing tests (`pg_bad_cases`, the kill harnesses, the job lease tests) still pass, and a second server can't write to a vault while the first holds its lease.
4. Spec 085's item 6 with vault counts like those that failed (about 20 per server) passes on a 64-connection Postgres.
5. Write latency at 1 and 100 vaults, against `main` without this, with the pass rule set before measuring.

## Out of scope
Pooling across processes, such as PgBouncer. The read pool, which spec 085 covers.

## Result
Measured on 2026-11-19 on Linux x86_64 (24 cores, shared with other workers, load average 3 to 30 during the runs). No shared Postgres was running, so every run used containers of this spec's own (podman, `pgvector/pgvector:0.8.7-pg18`): one with `max_connections` 64 on port 55493, one with 300 on 55494 so that `main` could open 100 and 200 vaults for the comparisons, and a short-lived one on 55432 for `pg_bad_cases`.

**Measured first, on `main`** (`tests/writers.rs`'s `pg_writers_measure`, one store, a pool limit of 10, a commit to each vault in turn for 20 rounds):

| vaults | connections (holding locks) | lease on a new vault, p50 | lease on an existing vault, p50 | commit p50 / p95 |
|---|---|---|---|---|
| 1 | 2 (1) | 44-74 ms | 3.0-5.6 ms | 0.55-1.13 / 0.61-2.17 ms |
| 10 | 11 (10) | 39-103 ms | 3.4-5.8 ms | 0.59-1.03 / 0.90-2.44 ms |
| 100 | 101 (100) | 41-61 ms | 3.0-6.4 ms | 0.65-0.96 / 1.03-1.98 ms |

On the 64-connection server, `main` couldn't open 100 vaults: the 63rd failed with `53300: too many clients`. A new vault's lease is mostly creating its schema. An existing vault's lease, which is what taking one back costs, is 3-6 ms, almost all of it opening a connection, and `Server::lease` adds `purge_incomplete`, a listing of the vault's `tmp` folder.

**Decision: share connections between vaults.** Each vault's advisory lock is taken on one of at most `MEMEX_PG_WRITER_CONNECTIONS` connections, and its commits run on that connection, as they ran on its own connection before. Why not the other two:
- **Releasing idle leases** lets a second server take a vault between writes, which the architecture's "one server process per vault" rules out. Its cost also lands on writes: 3-6 ms and a folder listing each time a lease is taken back, and with more vaults being written than the cap, writes in turn would take one back almost every time.
- **Capping writers** by taking and releasing leases as writes need them has both of those problems.
- **Sharing** keeps fencing exactly as it was: the lock and the commits are on the same connection, so a commit can't succeed after its lock is gone. What it costs is that vaults on one connection take turns, and losing one connection loses all of their leases at once. Spec 089 already takes each lease back separately, and the test below shows that working.

**What was built** (`crates/memex-store-postgres/src/store.rs`):
- `PoolLimits::writers`, by default `DEFAULT_WRITERS` = 8. The store opens a new writer connection while it has fewer than that, and then puts each new vault on the one holding the fewest vaults. A server with fewer vaults than the limit still gives each vault its own connection, as before.
- A vault's work waits its turn on its connection, in the order it came, up to `PoolLimits::wait` (`MEMEX_PG_WAIT_MS`). After that it gets `PgError::Backend("the vault's writer connection is busy with other vaults' writes, and didn't come free within N ms")`, which the API answers with 503 like spec 085's pool-full error. Nothing in core or the API changed.
- A lease holds while its token is current and its connection hasn't been seen closed. That is still a check with no round trip: the connection's closed flag is set when a call on it finds it closed. Closing or deleting a vault runs `pg_advisory_unlock` on the shared connection, and lets the connection go when no vault is left on it. A failure after taking the lock in `open_vault` unlocks it too, as the connection lives on. The store counts a vault on a connection under the list's lock, so a connection can't be let go between being chosen and being counted. `open_vault` still refuses a vault this store already holds, as session locks would otherwise stack on one connection.
- `compact` runs its `VACUUM`s on a pooled connection after checking the lease, as vacuuming changes no row and `VACUUM FULL` could otherwise hold up every vault sharing the writer connection.
- The setting `MEMEX_PG_WRITER_CONNECTIONS` (8, refusing 0). `docs/architecture.md`'s sizing rule is now each server's `MEMEX_PG_MAX_CONNECTIONS` plus the smaller of its writer limit and its vault count, so two servers with the defaults need at most 80. AGENTS.md has the setting and the tests.
- `crates/memex-store-postgres/tests/writers.rs`: the measurement above, items 1 and 2, the busy-connection error and fencing, and losing a shared connection. `tools/two_servers.py` runs item 4.

**The default.** 16 threads writing to 200 vaults, one event per commit, a pool of 10, two runs each:

| writer limit | 2,000 writes took | p50 | p95 | most connections |
|---|---|---|---|---|
| 1 | 1.69-2.06 s | 12.4-15.6 ms | 19.3-22.0 ms | 11 |
| 2 | 1.02-1.45 s | 7.3-10.7 ms | 10.5-18.0 ms | 12 |
| 4 | 0.59-0.66 s | 4.3-4.8 ms | 6.3-8.1 ms | 14 |
| 8 | 0.31-0.35 s | 2.2-2.5 ms | 3.2-4.9 ms | 18 |
| 16 | 0.24-0.27 s | 1.7-1.8 ms | 2.9-3.7 ms | 26 |

8 was chosen: it roughly doubles what 4 gets, and past 8 the gain is small for 8 more connections. In the same run `main` held 210 connections.

**Verification**
1. **Pass.** `pg_writers_200_vaults_stay_within_the_limit`: 200 vaults, a pool of 10, 8 writers, 16 threads writing 10 rounds to every vault and 16 readers on the pool. Sampled from `pg_stat_activity` every 5 ms, the store peaked at 18 connections, 8 of them holding locks, over 1,302-5,479 samples in 5 runs, including one beside the two servers of item 4. Every vault's head was the last write.
2. **Pass.** Every one of the 2,000 writes in item 1 succeeded, with p95 2.7-13.2 ms and a maximum of 106 ms. With 1 writer connection and a 300 ms wait, while another session's table lock held vault a's commit, vault b's write got the busy error after 300.03-300.26 ms in 3 runs. Once the lock was let go, a's commit finished (it took 504 ms) and b's next write took 1.3 ms. Nothing hung.
3. **Pass.** The library's Postgres suite, on the 64-connection container, at load 6.2 to 5.4: `pg_kill_postgres_100_times` (100 cycles, highest acknowledged 22,430, lost 0, partial 0, gaps 0, reopen failures 0, recovery p95 0.8 s), `pg_kill_memexvault_1000_times` (1,000 cycles, highest 231,250, lost 0, gaps 0), `pg_kill_jobs_200_times` (1,500 jobs, done 1,500, duplicates 0), both job-lease tests, `pg_contract`, and `pg_restart_postgres_under_running_servers` (B back 100 ms after the restart returned, A refused 191 times, 1 advisory lock held, all 210 acknowledged writes there). 11 passed. 5 failed for the environment reasons specs 085 and 089 recorded: the three harness children, the full-disk test, and `pg_bad_cases` on a port other than 55432. `pg_bad_cases` then passed on a container of this spec's own on 55432. In `writers.rs`, a second store was refused both vaults that the first held on one shared connection. After the first closed one of them, the second took that one and was still refused the other. Terminating the shared connection lost all three leases on it. All three were taken again on one new connection, and none of the old leases came back. A commit refused with a sequence gap left that connection clean for the next vault's commit.
4. **Pass.** `tools/two_servers.py` ran two release `memexvault serve` processes on the 64-connection container, each with `MEMEX_PG_MAX_CONNECTIONS=10` and the default 8 writer connections, in databases of their own. Each made a new vault every round up to 22, uploading to every vault and searching each round from 8 threads, then went on writing to all 22 for 40 minutes in total. Beside them ran the `memexvault` Postgres suites in a third database: `activity`, `audit`, `cache_tiers`, `chat`, `entities`, `extraction`, `mcp`, `meeting`, `pages`, `review` and `vector`, 43 tests, all passed. Then the store's `writers` tests, 4 passed, and `pool` tests, 4 passed. Over 45,985 samples each server peaked at **18 connections, 8 of them writers**, where spec 085's run reached 31 and 30. Neither server logged `too many clients`, and no request failed on a connection: 544,669 uploads answered 201, and 2,576,627 searches 200. The rest were 314,409 uploads answered 429, the upload limit on a vault's open jobs as the load outran its jobs, and 607 searches answered 503 "keyword index ... hasn't been built yet" on new vaults. The database server peaked at 64 of 64 once, during `pg_pool_latency_with_and_without_the_limit`, whose "no limit" round opens 48 connections of its own. That test and `pg_pool_survives_a_restart`, which needs `MEMEX_TEST_CONTAINER`, failed beside the servers. Afterwards all 6 pool tests passed on the same container at load 6 to 18, the restart test with `MEMEX_TEST_CONTAINER` set. Each server also logged one `42P01` warning, retried, when a worker saw a vault between `create_vault` registering it and its lease creating the schema. That order is the same on `main`. Load average 7 rising to 30.
5. **Pass**, by the rule set before measuring: over 3 alternating runs on the 300-connection container, this branch's median commit p50 and p95 must be within 15% of `main`'s, or within 0.3 ms, at 1 and 100 vaults, and with 16 threads over 200 vaults every write must succeed with p95 under 100 ms.

   | | `main` p50 / p95 (median of 3) | this spec p50 / p95 (median of 3) |
   |---|---|---|
   | 1 vault | 560 / 708 µs | 562 / 676 µs |
   | 100 vaults | 757 / 1,235 µs | 646 / 899 µs |
   | 16 threads, 200 vaults | 1.4-2.5 / 2.0-5.9 ms, 210 connections | 1.7-2.8 / 2.7-4.5 ms, 18 connections |

   A lease on an existing vault fell from 3.0-6.2 ms to 0.42-0.63 ms at 100 vaults, as most no longer open a connection. Load average 3 to 8.

**Other checks.** `cargo build --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check` and `cargo deny check licenses` pass. `cargo test --workspace` gives 1,013 passed, 0 failed and 123 ignored, in 83 binaries. No `web/` change.

**Follow-ups:** none needed. A long write on one vault, such as a big batch, holds up the other vaults on its connection for that long, and past `MEMEX_PG_WAIT_MS` they answer 503. The default of 8 keeps that rare, and `MEMEX_PG_WRITER_CONNECTIONS` raises it.
