---
id: 021
name: cache-tiers
description: Add configurable-size caches that speed up search and can never serve retracted content
---

# 021 cache-tiers

## Goal
Four cache tiers, each with a configurable byte cap. Cached queries have p95 ≤1 ms, caps hold under load, and retracted or
erased content is never served.

## Background
**Depends on:** 017, 020. Design: `docs/architecture.md`, section Caches.

## Approach
- `memex-cache`: moka for the query, inference and object tiers with byte weighers. An LRU file cache for page images and
  word tables.
- Keys built by pure functions, including projection watermark, version, model or DPI as listed, and the vault generation.
- Settings per tier in bytes, with 0 disabling a tier. Sensible defaults, with limits validated at startup.
- Hit, miss, eviction and size metrics in an API.

## Verification
1. Under a load 10× each cap, measured size never exceeds the cap by more than 5%. RSS is reported.
2. Cached query p95 ≤1 ms, and hit rate is recorded on a replayed golden-set workload.
3. Stale-read test: retract or edit an object, and the next read with the new watermark doesn't return old content. The same
   holds after a vault generation bump.
4. Every tier works with a cap of 0, and search results are identical with caches on and off across the golden set.
5. Disk cache: survives restart, a corrupt file is treated as a miss, and eviction keeps it within its cap.
6. Bad cases: invalid sizes are refused at startup, and a key from one vault can't return another vault's data.

## Out of scope
Distributed caches, the tantivy page cache (left to the OS) and `foyer` unless measurement justifies it.

## Result
Not started.
