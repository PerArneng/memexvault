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
Not started.
