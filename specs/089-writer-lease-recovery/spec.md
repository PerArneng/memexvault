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
(Filled in when the spec moves to done or dropped: what was built, decisions, numbers, follow-ups.)
