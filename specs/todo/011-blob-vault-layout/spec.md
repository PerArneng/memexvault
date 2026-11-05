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
Not started.
