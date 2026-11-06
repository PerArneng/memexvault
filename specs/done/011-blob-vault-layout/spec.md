---
id: 011
name: blob-vault-layout
description: "Store uploaded blobs as content-addressed files in folders and define the vault's disk layout"
---

# 011 blob-vault-layout

## Goal
Blobs live as plain files in folders, addressed by SHA-256, and no failure at any step can leave the store referencing a
missing or partial blob.

## Background
**Depends on:** 009. Design: `docs/architecture.md`, sections Backends, Consistency. This is the user's "blobs in folders on
disk" requirement, delivered as the default `BlobStore`.

## Approach
Default layout, recorded in `docs/architecture.md`. Tables live in Postgres, and everything on disk is below:

```
<data>/vaults/<vault-id>/
  blobs/ab/cd/<sha256>
  indexes/<kind>/gen-<n>/
  cache/
  tmp/
```

- `BlobStore` trait as the Backends section defines it, with no paths in its signatures and a reusable contract suite. The
  folder implementation's streaming put hashes while writing to `tmp/`, then does fsync, rename and directory fsync.
- Dedup: an existing hash is a no-op.
- Orphan sweep as a pure decision (age against grace period from `Clock`, not referenced) plus a thin edge.
- A `verify` operation that re-hashes files.
- An in-memory fake with injectable failures at every step.

## Verification
1. Fault injection at each step (write, fsync, rename, directory fsync, reference commit): 0 dangling references, and the
   retry succeeds.
2. Kill harness for 200 cycles during large uploads: no partial file is ever visible under `blobs/`.
3. A 2 GB blob goes through with peak RSS growth ≤64 MB.
4. Bad cases: a hash-shaped path traversal is refused; a corrupted file is reported by `verify`; a disk-full error surfaces
   and cleans up `tmp/`; a referenced blob is never swept.
5. Sweep tests use `ManualClock` and no sleeping.
6. The contract suite passes on the fake and the folder implementation, including range reads.

## Out of scope
Upload HTTP (spec 014), encryption at rest and a remote object storage adapter.

## Result
Built the `BlobStore` trait with no paths in its signatures, `BlobHash` in `memex-types` (64 lowercase hex only, so it's always a safe file name), `FsBlobs` in `memex-blob-fs` with its file operations behind an `FsOps` trait, the core wrapper `FsBlobStore` in the binary, `MemBlobStore` with injectable failures, a reusable contract suite, `Engine::record_source`, and the pure `sweepable` decision.

**Verification.**
1. Fault injection: a failure at write, fsync, rename and directory fsync on the real file system leaves `tmp/` clean, and the retry succeeds. Before the rename nothing is visible. A failed directory sync can leave the complete file in place, and the retry dedups onto it, which is safe since the name is its hash. Through the engine, a failed blob step leaves no Source row, and a failed reference commit leaves only an orphan. A loop of 400 rounds with mixed failures ends with every Source row's blob verified.
2. Kill harness, 200 `kill -9` cycles during 8 MiB writes: 0 partial files under `blobs/`, 0 acknowledged blobs lost, 687 blobs acknowledged in total, and 165 leftover temporary files purged, which shows the kills did land mid-write. This tests process crashes, not power loss.
3. 2 GiB blob in 16.1 s, with RSS going from 2.3 MB to 2.4 MB, target ≤64 MB of growth. Measured by sampling `ps` every 50 ms. The verify step re-hashed it correctly.
4. Bad cases: traversal attempts in vault names and hash text are refused; `verify` reports a tampered file; a simulated `ENOSPC` surfaces with its error code and cleans `tmp/`; a referenced blob is never sweepable however old; a stray non-hash file is neither listed nor swept.
5. Sweep tests use `ManualClock` with no sleeping.
6. The contract suite, including range reads, passes on the fake and the folder store.

**Decisions.** `purge_tmp` runs when taking a vault's writer lease, since no other put can be running then, and spec 012 has to wire that up. The sweep's loop lives with spec 030's operations, because it needs the set of referenced hashes from the store. Age comes from the file's modification time, which a backup restore can reset. That only delays a sweep, so it's safe. `docs/architecture.md` has the layout.
