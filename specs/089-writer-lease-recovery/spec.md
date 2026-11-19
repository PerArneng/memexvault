---
id: 089
name: writer-lease-recovery
description: "After Postgres restarts, a running server regains its vaults' writer leases and writes again, with no server restart"
---

# 089 writer-lease-recovery

**Depends on:** 032, 082.

## Goal
After Postgres restarts or a writer's connection drops, a running server regains each vault's writer lease and accepts writes again within seconds, with no server restart, and without ever letting two writers hold one vault.

## Background
Found while landing spec 082. `Server::lease` in `crates/memex-core/src/server.rs` caches each vault's `WriterLease` for the life of the process, and nothing drops it when `PgStore` reports `LeaseLost` (`crates/memex-store-postgres/src/store.rs`, `writer`). So once the writer's connection dies, every write to that vault fails until the server restarts. `docs/architecture.md`, Limits, says "Losing the connection loses the lease, so the writer stops until it regains it", but nothing regains it.

Measured on 2026-11-09 at load about 7, with `docker restart` of a throwaway Postgres under a running `memexvault serve`, on a binary built from `main` before spec 082 and on spec 082's release binary, with the same results on both:
- An upload to an existing vault answered 503 "a backend is unavailable, so try again" at 5, 30 and 60 s after Postgres was back. Reads and creating a new vault worked.
- The workers logged "can't claim a job: writer lease lost" about 17 times a second, 1,630 and 1,662 lines in about 95 s.
- After restarting the server, uploads answered 201.
- During a 400-page rendition, the job stayed `running` for over 10 minutes after the restart. After the server was restarted it finished in 72 s.

`pg_kill_postgres_100_times` passed throughout, losing 0 of 6,692 acknowledged writes, because it works on the store alone and reopens the vault itself. No test restarts Postgres under a running server.

## Approach
- When a write or claim gets `LeaseLost`, the server drops that vault's cached lease and takes a new one through `open_vault`, which takes the advisory lock again and refuses if another writer holds it. Recovery after a crash, such as `purge_incomplete`, runs as it does at startup.
- Retries back off, so an outage doesn't flood the log or the database. One line says the lease was lost and one says it was regained.
- Writes during the outage still fail with 503, and nothing acknowledged is lost.
- A job whose handler was running keeps spec 082's rules: its renewals fail, the fence refuses a stale result, and the job runs again once the lease is back.
- `docs/architecture.md` says what actually happens.

## Verification
1. On a throwaway Postgres under a running release server: restart Postgres, and uploads to an existing vault answer 201 again within a time set before measuring. Record that time and the load.
2. The same during a 400-page rendition: the job ends `done` with no server restart, with its attempts and time recorded.
3. Two servers on one database: after a restart, only one holds each vault's lease, and the other keeps refusing writes for it. Writes acknowledged before the restart are all still there.
4. During a 60 s outage, the log holds a bounded number of lines per vault, which is stated and checked.
5. A test that restarts Postgres under a running server is added to the Postgres suite, or to the landing checks if it needs a container, with `AGENTS.md` saying how to run it.
6. `pg_kill_postgres_100_times` and the Postgres suite still pass.

## Out of scope
Failing over to another Postgres server and pooling connections, which spec 085 covers.

## Result
Built:
- `Store::lease_held` asks whether a lease can still write, with no round trip: on Postgres, whether the writer's connection is still open and still this vault's. A dead connection shows as closed once a call on it has failed.
- `Server::lease` keeps each vault's lease as held or lost. A held lease whose connection has gone is dropped, with one WARN line, and the call that noticed tries `open_vault` straight away, which takes the advisory lock again or refuses with `WriterActive`, and then runs `purge_incomplete` as at startup. A failed try waits 250 ms, doubling to at most 5 s (`regain_delay_ms`), and calls in between get 503 "the vault's writer lease was lost and is being regained" without touching the database. Success logs one INFO line with the time and tries. If the cleanup fails, the lease is let go again, so the process never holds a lock it doesn't use. `Server::lease_lost` tells the workers to stay quiet about claims on that vault.
- The poll loops' warnings ("can't list vaults", "can't claim a job", "can't check the audit schedules" and an index step failing) go through a process-wide limiter, at most one line per 10 s each, with a count of what it held back. A job's keeper logs once when renewals start failing and once when they work again, where it used to log every 2 s.
- `docs/architecture.md`, Limits, says what happens. AGENTS.md says how to run the new test, and `MEMEX_TEST_CONTAINER_ENGINE=podman` lets it and `pg_kill_postgres` use podman.
- `tools/pg_restart.py` runs the real-server checks: it stops or restarts the Postgres container under a running `memexvault serve`, uploads every 250 ms, and counts the log.

Decisions: the first write after Postgres comes back can still answer 503, as only a failed call shows that a connection died. Probing the writer's connection before each write would cost a round trip, and the client retries anyway. A server that never held a vault's lease, such as a second server, tries for it on every write with no backoff, as before this spec, and so wins the race after a restart in the test below; either winner is fine, as the rule is only that one holds it.

Measured on 2026-11-18, with the release binary and a Postgres container of this worktree's own (podman, `pgvector/pgvector:0.8.7-pg18`, on loopback), on a 16-core machine shared with five other builds and test runs:

1. Uploads after a restart, against a limit of 15 s set before measuring. Plain `podman restart` at load 9.3: Postgres was ready 216 ms after the command returned, and an upload answered 201 279 ms after that, the one before it 503. After a 60 s outage at load 15.7: ready after 342 ms, 201 after 306 ms, one 503 first. Nothing acknowledged was lost (6 of 6 and 6 of 6). Before this spec, uploads still answered 503 60 s after the restart.
2. The 400-page rendition (a text PDF made with `gs`, 215 kB). A restart while it ran, at load 13.9 to 16.9: the job ended `done` in 1 attempt of 541.6 s, 535 s after Postgres was back, as the restart fell between renewals and its lease never ran out. A 60 s outage while it ran, longer than the 60 s job lease, at load 11.4 to 15.4: the keeper logged one failed renewal, the lease ran out, the first attempt ended "the worker stopped without finishing" after 66.8 s when the lease came back and a worker took the job again, and the second attempt finished `done` in 499.0 s. Before, the job stayed `running` over 10 minutes. No server restart either time.
3. `pg_restart_postgres_under_running_servers`: servers A and B on one database, A holding the vault and B refused. After `podman restart`, B wrote again 100 ms after the command returned, A was refused 188 and 189 times in 21 s in two runs and B 1 time, one advisory lock was held on the database, and all 207 and 208 acknowledged writes, 20 from before the restart, were there. Load 9.2 to 10.9.
4. The log bound, stated before measuring and checked by the tool: in an outage of S seconds, the server's own lines are at most 2 per vault (lost, regained), plus S/10 + 1 for each of the 4 limited warnings, plus 4 per running job. Lines written for a failed request (the access line and "backend unavailable") are counted apart, as they follow the clients' requests, one each per request. 60 s outage, one vault: 16 lines against 30 (7 "can't list vaults", 7 "can't check the audit schedules", 1 lost, 1 regained), and 479 for 240 refused uploads. The same during the rendition: 17 against 34, the one more being the keeper's. Before the limiter, the first run of the same outage logged 192 server lines (129 "can't list vaults" from the index projectors, 61 from the audit scheduler), and before this spec the workers logged about 17 lines a second.
5. The test is in the Postgres suite and needs `MEMEX_TEST_CONTAINER`, as AGENTS.md says. In memory, `crates/memex-core/tests/lease_recovery.rs` covers regaining after a write or a claim, a 60 s outage at a write every 100 ms (600 refused with 503, 15 to 17 tries reached the store, the write after it came back worked within 5 s, nothing acknowledged lost), another writer keeping the lease through 100 tries, and deleting a vault whose lease was lost.
6. The Postgres suite (`pg_` with `--ignored --test-threads=1`, release, on this worktree's container, 44 min at load 4.8 rising to 30): every test binary passed, 40 tests. In the library, 11 passed, including `pg_kill_postgres_100_times` (100 cycles in 12.9 min, highest acknowledged 10,160, lost 0, partial 0, gaps 0, reopen failures 0, recovery p95 13.9 s), `pg_kill_memexvault_1000_times` (1,000 cycles in 5.1 min, highest 37,568, lost 0), `pg_kill_jobs_200_times` (1,500 jobs, duplicates 0), both job-lease tests, and the new test (B back 202 ms after the restart returned, 205 acknowledged writes all there, load 12.3). 5 failed, none from this spec and all from the environment, in code this spec doesn't touch: `pg_child`, `pg_job_child` and `pg_job_child_long` are their harnesses' child processes and need the variables the harness sets. `pg_full_disk_returns_an_error_and_data_stays_readable` needs a tablespace on a small disk, as its ignore reason says. `pg_bad_cases` makes its unreachable URL by replacing the port 55432, so on this worktree's server at 55489 its "nothing listens there" connect succeeds.

The workspace: `cargo fmt --check`, `cargo build --workspace` and `cargo clippy --workspace --all-targets -- -D warnings` pass, and `cargo test --workspace` passed 1,000 tests in 81 binaries, 0 failed, 109 ignored, at load 11 to 14. No `web/` change.

Follow-ups: none needed for this spec. A dead pooled connection still fails one request each after a restart, which spec 085's pooling covers.
