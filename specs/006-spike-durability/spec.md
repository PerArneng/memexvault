---
id: 006
name: spike-durability
description: Prove group commit, the index watermark approach and crash behaviour before building on them
---

# 006 spike-durability

## Goal
Confirm that the consistency design works with the real libraries. Pass: at least 1,000 events per second with durable
group commit, and 1,000 `kill -9` runs that lose no acknowledged commit and never leave an index ahead of the log.

## Background
**Depends on:** none. Design: `docs/architecture.md`, section Consistency.

The design relies on assumptions about Postgres and `tantivy` 0.27: commit cost and batching, point-read latency, a tantivy
commit payload that can hold a watermark, and filtering on JSON fields for dynamic entity attributes. If one is false the design must
change before code depends on it.

## Approach
- Throwaway crate in this spec's folder.
- Compare pure-Rust Postgres clients, and record the choice and how it fits the synchronous `Store` trait.
- A single writer loop batches events into one durable SQL transaction and acknowledges them after commit. Measure
  throughput and latency at several batch sizes and event sizes, over a Unix socket and TCP on localhost.
- Measure batched point reads at 200k rows, and at 50k for growth, against the exact-lookup target.
- Use the Postgres image from spec 004's `compose.yaml`.
- Tantivy: round-trip a watermark in the commit payload, reopen, and resume. Test JSON field term, range and fast-field
  filters on 200k documents with dynamic keys.
- Crash harness: a parent starts the child writer, records acknowledgements from a pipe, kills it at a random moment, and
  checks the store on restart. It repeats this 1,000 times, then 100 times killing Postgres instead.

## Verification
1. Throughput and commit latency table. ≥1,000 events/s with durable commits and commit latency p95 reported.
2. In all kill runs: 0 acknowledged events lost, 0 partial transactions, 0 tantivy watermarks ahead of the log, and 0
   reopen failures.
3. JSON-field filters: p95 ≤15 ms on 200k documents, with a projection to 1M, or the alternative design recorded. Batched point reads meet ≤2 ms p95, or
   a revised target is recorded.
4. The Result states plainly that `kill -9` tests process crashes, not power loss. Power-loss safety depends on Postgres
   settings and the OS's fsync behaviour. Check and record what macOS `fsync` versus `F_FULLFSYNC` means for `wal_sync_method`, and
   what Docker's disk layer does, as these may change the commit-rate number.
5. `docs/architecture.md` is updated with anything found.

## Out of scope
Event schemas, product crates and real power-cut testing.

## Result
**The consistency design holds. Decisions: the `postgres` crate as the client, group commit with batches of at least 10 events, and tantivy JSON fields as designed.** No assumption failed, but commits inside Docker on macOS aren't power-safe, as explained below. Raw output is in `results.txt`.

**Durable commits.** One transaction with the event rows and materialized rows, `synchronous_commit=on`, `fsync=on`, TCP to Docker on this machine, closed loop for 8 s.

| Batch | 256-byte events/s | commit p50 / p95 | 4 KB events/s | commit p50 / p95 |
|---|---|---|---|---|
| 1 | 394 | 2.2 / 4.0 ms | 363 | 2.4 / 4.3 ms |
| 10 | 3,528 | 2.6 / 4.3 ms | 2,596 | 3.6 / 5.8 ms |
| 100 | 24,663 | 3.7 / 6.2 ms | 10,746 | 8.6 / 14.0 ms |
| 1,000 | 78,550 | 12.1 / 17.3 ms | 17,040 | 58.8 / 65.7 ms |

So a commit per event misses 1,000 events/s, and a batch of 10 clears it by 3.5 times. Group commit is required, as designed.

**Crash harness.** A child writer commits batches of 1 to 20 events, acknowledges each through a pipe, and commits tantivy with a watermark every 5 batches. A parent kills it at a random moment and checks everything.

| Runs | Killed | Acknowledged lost | Partial transactions | Sequence gaps | Watermark ahead of log | Reopen failures | Index doc count wrong |
|---|---|---|---|---|---|---|---|
| 1,000 | Writer, `kill -9` | 0 | 0 | 0 | 0 | 0 | 0 |
| 100 | Postgres, `docker kill -s KILL` | 0 | 0 | 0 | 0 | 0 | 0 |

The log reached 94,598 and then 107,279 acknowledged events. Across the writer kills, 39,490 events had been above the index watermark and were replayed on restart, so resuming really was exercised. Postgres recovery p95 was 0.32 s.

**This tests process crashes, not power loss.** Power-loss safety depends on Postgres settings and whether the OS's flush reaches the drive, and these measurements show that on this machine it doesn't:

| Measurement | Result |
|---|---|
| Linux VM, `pg_test_fsync`, `fdatasync` (Postgres's `wal_sync_method` here) | 3,959 per second, 253 µs |
| macOS host, `fsync` | p50 0.05 ms |
| macOS host, `F_FULLFSYNC` | p50 5.1 ms, p95 9.9 ms |

The VM's flush is 20 times faster than a real flush on this SSD, so a commit in Docker Desktop very likely stops short of the physical drive. I couldn't read Docker's disk settings to confirm how it syncs, since macOS denied access to its settings file. macOS's plain `fsync` doesn't reach the drive at all, which is why Postgres offers `wal_sync_method=fsync_writethrough`. It isn't available in this Linux VM. In practice:
- Docker on macOS is for development. An acknowledged commit survives a crash of Postgres or the VM's processes, and may not survive a power cut.
- Native Postgres on macOS needs `wal_sync_method=fsync_writethrough`, which costs about 5 ms per commit. Groups keep this affordable: about 200 commits per second at batch 100 is 20,000 events/s.
- On Linux with a real disk, the default `fdatasync` is right. A cloud or virtual disk should be checked with `pg_test_fsync`, since one at 250 µs per flush is either a very good drive or not flushing.

The startup check only sees `fsync` and `synchronous_commit`, and can't detect any of this. Spec 058 raises the question of what to tell operators.

**Point reads** (1 KB rows, 2,000 samples, TCP):

| Rows | Single p50 / p95 | Batch of 50 p50 / p95 |
|---|---|---|
| 50,000 | 0.41 / 0.77 ms | 0.73 / 1.46 ms |
| 200,000 | 0.41 / 0.82 ms | 0.75 / 1.54 ms |

Target ≤2 ms p95, met. Growth from 50k to 200k is about 5%, so 1M should stay under 2 ms when the table fits in memory.

**Tantivy 0.27.** A commit payload round-trips through a fresh open: `prepare_commit`, `set_payload`, `commit`, and `load_metas().payload` on reopening. JSON fields with dynamic keys, on 200,000 documents across three made-up types, with no schema change per type:

| Filter | Hits | p95 at 50k | p95 at 200k |
|---|---|---|---|
| Term | 13,334 | 0.05 ms | 0.10 ms |
| Rare term | 67 | 0.01 | 0.01 |
| Float range | 1,400 | 0.06 | 0.15 |
| Integer range | 3,334 | 0.04 | 0.14 |
| Term, range and text combined | 5,333 | 0.14 | 0.41 |

Hit counts match by hand calculation from the generator. Target ≤15 ms. Time grows linearly with data, so about 2 ms at 1M, far inside it. Fields used `raw` tokenization and fast fields, so spec 023 should use the same options. These documents are small, and full-text queries over real segments are spec 018's to measure.

**Client.** `postgres` 0.19, the synchronous wrapper over `tokio-postgres`: pure Rust, and `otool -L` lists system libraries only, with no `libpq` or OpenSSL in `cargo tree`. It fits the synchronous `Store` trait directly, and keeps its Tokio runtime inside the edge crate. `sqlx` wasn't measured: it's async, so it would need a bridge, and its compile-time SQL checks need a database at build time. TLS isn't set up yet (`NoTls`), which suits the same-machine default, and spec 059 tracks it.

**Not measured:** a Unix socket, because Docker Desktop on macOS doesn't share one with the host. The TCP numbers include Docker's port forwarding, so they're on the pessimistic side.

**Verification.** 1: table above, with ≥1,000 events/s at batch 10 or more. 2: all zeros in both kill modes. 3: met. 4: stated above, including `F_FULLFSYNC` and Docker. 5: `docs/architecture.md` updated.
