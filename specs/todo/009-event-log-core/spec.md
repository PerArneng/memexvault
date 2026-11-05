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
Not started.
