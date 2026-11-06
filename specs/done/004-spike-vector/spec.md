---
id: 004
name: spike-vector
description: Choose the vector index by measuring recall and latency at the test scale with filters and deletes
---

# 004 spike-vector

## Goal
Pick the design for `memex-vector`, preferring pgvector. Pass: at 200k vectors, recall@50 ≥0.95 and p95 ≤15 ms, including with 1%, 10% and 50%
filters and 20% deleted vectors.

## Background
**Depends on:** none. Design: `docs/architecture.md`, sections Search, Targets, Consistency.

pgvector is preferred because vectors then live in the same database and transaction as everything else, with no
separate index files to keep consistent. The other options exist in case it misses the pass mark. Those are: flat scan over binary codes with int8 rescoring (simple, our own code, predictable filters and deletes);
`usearch` 2.26 (vendored C++); `hnsw_rs` 0.3.5 (pure Rust). Filtering by metadata and deleting are the usual weak points
of graph indexes, and flat scan may be good enough at this scale, so they are compared head to head.

## Approach
- Throwaway crate in this spec's folder. Add a root `compose.yaml` with a pinned Postgres and pgvector image for development.
  Docker is a development convenience, not a product requirement.
- Test pgvector's HNSW index with its filtered-scan settings, and record the settings used.
- Dataset: 200k real embeddings of public text if practical, otherwise a public embedding dataset. Random vectors aren't
  acceptable, because they make recall meaningless. Test 384 and 768 dimensions.
- Ground truth by exact search with full-precision floats.
- Also run at 50k, and project to 1M as the Test scale rule says. Exact scan and graph indexes grow differently, so the
  projection is part of the comparison.
- Run 1,000 queries per condition, with 1 and 8 query threads, and once more while a writer inserts 100 vectors per second.
- Also measure build time, incremental insert, disk size, RSS, and restart time with mmap.

## Verification
1. Results table per option and condition.
2. The chosen option has recall@50 ≥0.95 and p95 ≤15 ms in every filter and delete condition, on 8 threads.
3. Restart with the index on disk is ≤1 s without a rebuild.
4. Recorded answer on how the watermark and generation flip fit the chosen design, including a side-by-side model migration.
5. Total test data stays within 20 GB, and the size per option is recorded.
6. `docs/architecture.md` records the choice, quantization and memory budget, including Postgres memory settings.

## Out of scope
Embedding model choice (spec 005), the projector (spec 019), multi-vector indexes and billion-scale designs.

## Result
**Decision: pgvector 0.8.7 on Postgres 18, `halfvec(384)` with `halfvec_ip_ops`, HNSW `m=16, ef_construction=128`, and a search plan chosen by estimated matches.** It misses the pass mark in one band, a filter matching roughly 6 to 20% of rows, so the target is revised there and spec 057 raises it with the user. The reason to keep pgvector despite the miss is that vectors commit in the same transaction as everything else, with no second store to keep consistent.

Dataset: 200,000 paragraphs from the English, Swedish, German and Spanish Wikipedias (50,000 each, `wikimedia/wikipedia` 20231101), embedded by spec 005's model at 128 tokens, 82 passages/s. 1,000 queries are first sentences of other articles. Ground truth is exact search on full floats. Filters use an independent hash bucket, so they aren't correlated with content. Postgres ran in Docker Desktop (10 CPUs, 7.65 GB), and the client used TCP through Docker's port forwarding, so a native server should do somewhat better. Raw runs are in `results.txt`.

**Final configuration at 200k, 8 query threads:**

| Condition | Plan | Recall@50 | p50 | p95 | Target met |
|---|---|---|---|---|---|
| No filter | HNSW `ef_search=300` | 0.974 | 7.0 ms | 9.6 ms | yes |
| 50% | HNSW 300, `relaxed_order` | 0.957 | 6.9 | 9.9 | yes |
| 20% | HNSW 200, relaxed | 0.939 | 9.0 | 13.6 | **recall misses** |
| 10% | HNSW 100, relaxed | 0.933 | 11.4 | 16.0 | **both miss** |
| 5% (10k rows) | Exact via B-tree | 0.999 | 7.8 | 11.4 | yes |
| 2% | Exact | 0.999 | 3.9 | 6.0 | yes |
| 1% | Exact | 0.999 | 3.1 | 5.2 | yes |
| No filter, 20% deleted | HNSW 300 | 0.970 | 6.7 | 10.7 | yes |
| 50%, 20% deleted | HNSW 300 | 0.950 | 7.1 | 12.0 | yes |
| 10%, 20% deleted | HNSW 100 | 0.940 | 13.2 | 18.3 | **both miss** |
| 1%, 20% deleted | Exact | 0.999 | 2.7 | 4.5 | yes |
| After `VACUUM`: no filter, 50%, 10%, 1% | same | 0.977, 0.962, 0.942, 0.999 | 6.9, 6.8, 11.1, 2.6 | 9.5, 9.9, 15.1, 4.4 | as above |
| Writer inserting at 99/s: no filter, 50%, 10%, 1% | same | 0.968, 0.957, 0.934, 0.999 | 7.3, 7.0, 12.1, 3.0 | 11.3, 9.6, 17.2, 5.1 | as above |

One query thread at 200k: p95 7.4, 7.4, 13.6 and 3.0 ms for no filter, 50%, 10% and 1%, with the same recall. So at one thread the 10% case meets latency but not recall.

**50k:** 0.987, 0.978, 0.968 and 1.000 recall with p95 7.7, 7.1, 12.1 and 1.9 ms. All pass, and exact at 10% (5,000 rows) is 0.999 at 6.1 ms. **Growth from 50k to 200k:** p95 up 25 to 35%, and recall down 0.013 to 0.035.

**Why a chosen plan.** Left to itself, the planner stays on the graph at 2 and 5%, giving p95 36 and 26 ms. At 1% it picks exact on its own. Forcing exact (`SET LOCAL enable_indexscan = off`) when estimated matches are at most about 12,000 rows keeps p95 under 15 ms, whatever the vault size. Above that, use the graph. An exact scan costs about 0.75 µs per row at one thread and is slower under load, 20,000 rows taking p95 25 ms on 8 threads, so 10% can't go exact at this scale. More parallel workers made it worse under load, and over-fetching with a materialized CTE gave the same recall.

**Compared.**

| Option at 200k, 8 threads | No filter | 50% | 10% | 1% | Size |
|---|---|---|---|---|---|
| pgvector `vector` cosine, ef 200 to 300 | 0.957, 8.5 ms | 0.931, 8.3 ms | 0.94, 21 ms | 1.0, 5.2 ms | index 409 MB, 34 s build |
| **pgvector `halfvec` inner product** | 0.974, 9.6 | 0.957, 9.9 | 0.933, 16.0 | 0.999, 5.2 | index 223 MB, 24 s build |
| `usearch` f32 in-process, ef 200 | 0.936, 2.0 | 0.957, 2.9 | **0.987, 11.3** | 0.999, **57** | 336 MB, 24 s build |
| `usearch` f32, ef 400 | 0.973, 3.1 | 0.984, 5.7 | 0.997, 21.9 | 1.000, 84 | same |
| Flat, full int8 scan | 0.953, 65 ms | not run | 0.959, 7.5 | 0.967, 1.3 | 76 MB plus ids |
| Flat, binary codes with 2,000 rescored | 0.353, 1.7 | not run | 0.625, 0.7 | not run | 9 MB plus 76 MB |

`usearch` is 3 to 5 times faster unfiltered, and the only one passing at 10%, but it fails at 1%, since in-graph filtering scans far when matches are rare, and it would need an exact path as well. Choosing it means a second durable store with its own manifest, and a watermark across two systems. It's the first alternative if spec 057 or spec 033 demands it, and `VectorIndex` keeps that open. Binary codes are unusable on this model: sign bits lose too much. Not run: `hnsw_rs`, since `usearch` already stood in for an embedded graph; 768 dimensions, since spec 005 rejected the only 768-dimension model on latency, so this was not needed.

**Verification.**
1. Tables above.
2. **Partly failed:** recall ≥0.95 and p95 ≤15 ms hold everywhere except the 6 to 20% band. Revised target in force for that band: recall ≥0.93 and p95 ≤20 ms. Spec 057 raises it.
3. Restart: Postgres accepted connections 0.8 s after `docker restart`, with no rebuild. The first 1,000 queries had p95 8.2 ms, but the VM's page cache had stayed warm, so this isn't a cold-disk number.
4. Watermark and generations: a `vector_state` table holds the watermark, and it commits in the same transaction as the vector rows, so it can't disagree with them. For a model migration, each generation has its own table `embeddings_gen<n> (segment_id, bucket columns, embedding halfvec(d))` with its own HNSW index, which can have a different dimension. A projector backfills the new one while search keeps using the old, a row in `vector_state` flips reads, and the old table is dropped later. This is designed, not run.
5. Test data: Docker volume 2.57 GB, corpus 2.4 GB, vectors and ground truth 0.63 GB, about 5.6 GB against the 20 GB cap. Per option: `vector` 743 MB, `halfvec` 422 MB, `usearch` 336 MB.
6. `docs/architecture.md` records the choice.

**Other costs measured.**
- `halfvec` load by `COPY`: 12.4 s. Inserting 1,000 rows in one statement took 2.05 s with the index in place, about 490 rows/s, far above embedding's 40 per second.
- Deleting 40,055 rows took 0.3 s, and **`VACUUM` then took 6 min 18 s**, since it repairs the HNSW graph. Reads weren't blocked in this run and stayed healthy before and during. Spec 019 and spec 030 must run it in the background and expect it to take long.
- Postgres memory: 1.5 GB resident at load in the container, 442 MB just after a restart. Settings: `shared_buffers=2GB`, `effective_cache_size=5GB`, `maintenance_work_mem=1500MB` for builds, `shm_size` above that, or the build fails with `could not resize shared memory segment`.

**Projection to 1M, not measured.** Index about 1.1 GB and table about 0.9 GB, so `shared_buffers` should be at least 2.5 GB. Unfiltered p95 about 12 to 13 ms. Recall at `ef_search=300` falls by about 0.015 per 4× of data, to about 0.96, which is still passing but with `ef_search` possibly needing 400. The 10% case would match 100,000 rows, and recall should fall further, to perhaps 0.90. That misses even the revised target, so spec 057 includes it, and only spec 033 can prove it.
