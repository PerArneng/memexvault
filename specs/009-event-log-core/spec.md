---
id: 009
name: event-log-core
description: Build the event log, typed mutations and replay as pure code tested against an in-memory store
---

# 009 event-log-core

## Goal
The system of record's logic in pure crates: `Store`, event envelope, typed mutations, reducers, and replay. It
must run entirely in memory, and replaying 100k random events must reproduce an identical state.

## Background
**Depends on:** 008. Design: `docs/architecture.md`, sections Backends, Data model, Consistency.

## Approach
- `Store` trait as the Backends section defines it: typed snapshot reads, batched lookups, atomic `commit(event, mutations)`,
  `open_vault` with a writer lease, and `compact`. It's synchronous. `MemStore` fake with the same semantics and a store-call
  counter.
- A contract suite written as a reusable function over any `Store`, so spec 032 runs it unchanged.
- Event envelope: sequence, vault, actor, time, kind, schema version, payload. The payload is versioned JSON, which
  Postgres stores as JSONB.
- A pure reducer turns an event into typed row mutations. Row types and the header for the first object kinds.
- Replay from the log into an empty store, and a canonical dump with a hash for comparisons.
- Upcasters with golden fixtures of old event versions.

## Verification
1. A property test generates 100k random events, applies them, replays into a fresh store and compares dump hashes.
2. The canonical dump is stable: the same rows in any insertion order give the same hash, including empty and non-ASCII values.
3. Bad cases: an aborted transaction leaves no trace; a corrupt or unknown-version event returns an error with no partial
   state; a non-contiguous sequence is refused.
4. A golden fixture of version 1 events replays through an upcaster to the same dump.
5. Tests need no file, clock or network, and `cargo clippy -D warnings` passes with the IO lint on.
6. Applying one event makes exactly one `commit` call, shown by the store-call counter.

## Out of scope
Real persistence (spec 032), the proposal flow (spec 013), and snapshotting.

## Result
Built the system of record's logic in pure crates: `Store` and `Snapshot` traits, the event envelope, row types and typed mutations for the first kinds (Source, Page, Edge), the reducer, replay, the canonical dump with sha256, upcasters, `Engine::apply`, the `MemStore` fake with a call counter and an injected commit failure, and the contract suite as public functions in `memex_core::testing::contract`, for spec 032 to run unchanged.

`Store` has a generic associated `Snap` type, so a Postgres snapshot can hold a repeatable-read transaction with no `Box<dyn>`. Later kinds (Segment, Assertion and so on) are added by the specs that need them.

**Verification.** `cargo test -p memex-core`: 34 tests pass in 0.3 s in release mode.
1. 100,000 random events go through the engine and replay into a fresh store with equal dump hashes. The generator uses a small id range and a test asserts over 20,000 are applied and over 20,000 refused, so edits, stale versions, deletes and duplicates all occur. A fifth of `source.recorded` uses are version 1 to exercise the upcaster. Two seeds give different hashes.
2. Canonical dump: rows in different orders give the same bytes, at unit level and through two stores created in different orders. Empty, non-ASCII, quote and newline values are covered.
3. Bad cases: an aborted commit leaves no trace and doesn't use up a sequence number. An unknown kind, a future or zero version, and a corrupt payload all return errors with no state change. A delete of a missing row refuses the whole commit. A non-contiguous sequence is refused by the store, and replay reports it as a gap. Replay stops at a bad event and keeps only what came before.
4. Golden fixture: `events_v1.jsonl` replays through the upcaster to the same dump as `events_current.jsonl`, and the hash `15bc459f…` is pinned. A test also checks the dump's content directly, and the log keeps version 1 as written.
5. No file, clock or network use, confirmed by grep, and `cargo clippy --workspace --all-targets -- -D warnings` passes with the IO lint on.
6. One event makes exactly one `commit`, shown by the counter, and a refused event makes none. `get_many` of 20 keys is one read.

**Note for spec 032.** A row's `id` is unique per table, not per vault. The fixed vault name `random-source` in `random_replay` means a Postgres run needs a fresh vault per run.
