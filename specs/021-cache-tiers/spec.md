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
Built, with one target missed. Item 2's cached p95 is over 1 ms at 200k on this busy machine, in 3 of 4 runs with 1 client and every run with 8. The target hasn't moved, so the user decides what follows.

**Built:**
- `memex-cache`: `RamTier` on moka 0.12 and `DiskTier`, an LRU of files. moka and tagptr bring no new licenses.
- `memex-core/src/cache.rs`: the `Caches` trait, `NoCaches`, and pure key functions. `memexvault/src/cache_edge.rs` is `TieredCaches`, wired only into `serve`.
- Search goes through `cached_search`. The query tier sits in front of it, the inference tier holds query embeddings and rerank scores, and the fence reads segments, entities and pages through the objects tier.
- `page_image` and word tables go through the disk tier.
- `Store::cache_marks`, `Store::bump_cache_generation`, `Snapshot::row_stamps`, and `VectorIndex::live_mark`. The first three are in the Store contract suite, so they run on Postgres too.
- `Reranker::model` and `Rasterizer::renderer`, both required, and `ServerEdges::Caches`.
- `Server::bump_cache_generation`, for spec 030 to call, and `GET /api/v1/caches`, for server admins.
- Settings `MEMEX_CACHE_{QUERY,INFERENCE,OBJECT,DISK}_BYTES`, by default 256 MiB, 64 MiB, 256 MiB and 2 GiB. 0 turns a tier off. Anything else must be from 1 MiB up to 256 GiB, or 64 TiB for disk, or startup refuses it.
- Vault migration 11: `cache_generation`. It was 10 until spec 25 landed first.

**Verification.** Timings are from a machine shared with three other workers, with a load average of 12 to 50.

| Item | Result | Measured |
|---|---|---|
| 1. Caps under 10× load | Passed | At 256 MiB per tier, 4 writers and a sampler: highest size 1.0000× the cap on every tier, against a limit of 1.05×. Each wrote 10.0× its cap. RSS grew 860 MB for 768 MB of RAM caps (1.12×), and 226 MB for 192 MB at 64 MiB per tier (1.18×). |
| 2. Cached p95 ≤1 ms | **Failed at 200k** | See below. Hit rate on the replay 100% after the cold pass, 97.83% over the whole run. |
| 3. Stale reads | Passed | In memory through a real `Server` and tiers: retract, edit, a reused version number, a bump, `as_of`. Stamps, marks and the vector mark also on Postgres. |
| 4. Cap 0, same results | Passed | 200k: 230 questions, twice each, in hybrid, lexical and vector with all tiers on, each tier at 0 in turn and all at 0, and hybrid+rerank with all on and all at 0: 0 differences in 7,820 whole-package comparisons. Disk at 0 and each mix also pass in memory. |
| 5. Disk tier | Passed | Tests in `memex-cache`: restart, four kinds of corruption, a file copied from another key, a crash's half-written file, eviction, and four writers at once. Through the server: images and word tables hit until a bump, which removes their files. |
| 6. Bad cases | Passed | The binary exits with code 2 and names the setting for 4 bad values, and the parser refuses 8 bad forms on each tier. Two vaults with one query get their own answers in every tier. |

Item 2, in ms, with the rule set before measuring (p95 ≤1 at 200k):

| Corpus, run | Clients | p50 | p95 | p99 |
|---|---|---|---|---|
| 200k, run 1 | 1 | 0.359 | **1.149** | 1.569 |
| 200k, run 1 | 8 | 0.742 | **1.642** | 2.181 |
| 200k, run 2 | 1 | 0.418 | **1.176** | 1.639 |
| 200k, run 2 | 8 | 0.757 | **1.715** | 2.253 |
| 200k, run 3 | 1 | 0.368 | **1.361** | 1.679 |
| 200k, run 3 | 8 | 0.740 | **1.626** | 2.184 |
| 200k, run 4 | 1 | 0.417 | 0.772 | 1.429 |
| 200k, run 4 | 8 | 0.749 | **1.807** | 2.217 |
| 50k | 1 | 0.382 | 0.492 | 0.587 |
| 50k | 8 | 0.691 | **1.416** | 1.878 |

- **Almost all of a hit is one Postgres round trip through Docker Desktop.** The marks read alone took p50 0.330 to 0.397 ms and p95 0.580 to 1.338 ms in runs 2 to 4, close to the whole hit in each. A hit's lookup and clone are small beside it.
- **An earlier build missed for a fixable reason.** With a second round trip for the vector watermark, 50k measured p50 0.874 and p95 1.105 ms with 1 client, and p95 3.175 ms with 8. That watermark now comes from memory.
- **Corpus size shouldn't change this.** A hit reads `max(seq)` by primary key and doesn't touch the indexes, so the 50k to 200k gap is more likely load than growth, and 1M should cost the same. That's a projection. The load wasn't held equal, so this data can't separate the two.
- **Not measured here:** these times are from `cached_search`. A request over HTTP also pays for its token check.

A miss costs slightly more with the objects tier on. At 200k with 1 client, p95 without caches and on a cold pass: 27.8 and 30.0 ms, 33.6 and 35.8 ms, 29.4 and 29.3 ms, 28.1 and 27.5 ms.

Definition of done, on the code of the final commit: `cargo build --workspace`, `cargo test --workspace` (591 passed, 0 failed, 63 ignored), clippy, `cargo fmt --check` and `cargo deny check licenses` all pass. The Postgres command passes 22 of 22 on a fresh database. `web/` didn't change.

**Not run:**
- `pg_bad_cases` and `pg_kill_postgres`, which disturb the shared server, and `pg_child`, `pg_job_child` and `pg_full_disk`. A Postgres failure during a hit should go through the marks read, which falls back to the uncached search and its 503. That's tested only with a deleted vault, not a killed server, so `pg_kill_postgres` is worth running at landing. A full disk fails a cache write as a logged miss, with no test on a really full disk.
- `memexvault serve` with the real tiers, end to end, as it needs installed models. Only the startup refusal ran through the binary.

**Decisions:**
- **The query key holds the log's head as well as the watermarks.** With watermarks alone, a package could outlive a retraction until the index caught up, against this spec's goal. The cost is that any event misses, so hits need a quiet log. Hit rate during ingest wasn't measured and should be near 0.
- **The objects key is a stamp of the row, not its version,** changing the design's "Id, version". `inverse_of` restores a row with its old version, and the next edit reuses that number for new content, so (id, version) could bring back reverted text. Postgres computes `md5(row::text)`, so the row stays there. A test covers it.
- **The generation lives in the vault's schema,** read in the same statement as the head, so a hit needs one store call. A bump needs the writer lease and isn't an event.
- **A hit doesn't call `vault_exists`.** Marks can't be read for a deleted vault, and access is still checked first.
- **Rerank scores are cached per batch,** as a batch pads to its longest passage and scores could differ by batch.
- **Eviction is LRU,** as TinyLFU would keep popular dead keys and refuse new ones. Writes take turns, and an entry over a twentieth of a tier's cap isn't kept, which together bound the overshoot at 5%. Weights are debug text length plus 512 bytes per entry, raised from 256 after the first RSS measurement of 1.31×.
- **Not kept:** degraded answers, `debug` and `min_seq` searches. `as_of` by time keeps the head in its key, as a time can be ahead of the log.

**What the tiers bring so far.** On the golden replay the query tier answers nearly everything. The inference tier got 0 hits there, as the query tier answers first, and the objects tier about 2.8% at 200k, as questions rarely share rows. Neither has shown its value on this workload, and real traffic with overlapping hits may differ.

**Follow-ups:**
- The user decides on item 2: re-measure on a quiet machine or native Postgres, or review the target as spec 062 does for search. Spec 031's acceptance run is a natural place for the first.
- Spec 030 must call `bump_cache_generation` after removing data. A search running across a bump can store an entry under the old generation after the purge. It can't be reached, but it stays in RAM or on disk until evicted, so spec 030 should decide whether that's acceptable for erasure.
- Any branch adding an `impl ServerEdges` needs `type Caches = memex_core::NoCaches;`, and any `Reranker` or `Rasterizer` needs its new method.
