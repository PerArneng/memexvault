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
(Filled in when the spec moves to done or dropped: what was built, decisions, numbers, follow-ups.)
