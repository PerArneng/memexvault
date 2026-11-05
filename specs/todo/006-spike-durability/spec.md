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
- Measure batched point reads at 1M rows against the exact-lookup target.
- Use the Postgres image from spec 004's `compose.yaml`.
- Tantivy: round-trip a watermark in the commit payload, reopen, and resume. Test JSON field term, range and fast-field
  filters on 1M documents with dynamic keys.
- Crash harness: a parent starts the child writer, records acknowledgements from a pipe, kills it at a random moment, and
  checks the store on restart. It repeats this 1,000 times, then 100 times killing Postgres instead.

## Verification
1. Throughput and commit latency table. ≥1,000 events/s with durable commits and commit latency p95 reported.
2. In all kill runs: 0 acknowledged events lost, 0 partial transactions, 0 tantivy watermarks ahead of the log, and 0
   reopen failures.
3. JSON-field filters: p95 ≤15 ms on 1M documents, or the alternative design recorded. Batched point reads meet ≤2 ms p95, or
   a revised target is recorded.
4. The Result states plainly that `kill -9` tests process crashes, not power loss. Power-loss safety depends on Postgres
   settings and the OS's fsync behaviour. Check and record what macOS `fsync` versus `F_FULLFSYNC` means for `wal_sync_method`, and
   what Docker's disk layer does, as these may change the commit-rate number.
5. `docs/architecture.md` is updated with anything found.

## Out of scope
Event schemas, product crates and real power-cut testing.

## Result
Not started.
