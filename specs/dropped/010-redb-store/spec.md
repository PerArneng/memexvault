---
id: 010
name: redb-store
description: Implement the store on redb with group commit and pass the shared contract and crash suites
---

# 010 redb-store

> **Dropped (2026-11-05):** Replaced before any work started: the user chose PostgreSQL as the real database, so spec 032 builds the store instead of redb.

## Goal
A durable `KvStore` on redb with one file per vault, behaving exactly like the fake and surviving process crashes.

## Background
**Depends on:** 009, and results from 006. Design: `docs/architecture.md`, section Consistency.

## Approach
- `memex-store-redb` with a thin wrapper, plus the trait implementation in the binary crate as the orphan rule requires.
- Run the contract suite from spec 009 against redb. Changes to it apply to `MemKv` too.
- A group-commit writer: one writer, batched transactions, acknowledged only after durable commit. The batching decision
  is a pure function.
- Open checks: file lock, format version, and a clear error when a second process opens the vault.

## Verification
1. The same contract suite passes on both implementations.
2. The kill harness from spec 006, run against the product crate for 1,000 cycles: 0 lost acknowledged events and 0 reopen
   failures.
3. Replay of 100k events through redb has the same dump hash as through `MemKv`.
4. ≥1,000 events/s sustained for 60 s, or the revised target recorded by spec 006.
5. Bad cases: disk-full simulated by a small volume returns an error and stays readable; a second open is refused; an
   unknown format version is refused.

## Out of scope
Blobs (spec 011), backup (spec 030) and replication.

## Result
Not started.
