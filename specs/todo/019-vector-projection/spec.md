---
id: 019
name: vector-projection
description: Embed content in-process and project it into the vector index without re-embedding on rebuild
---

# 019 vector-projection

## Goal
A vector index fed only by a projector, using stored embeddings, so a rebuild needs no embedding work, and an embedding
model change is a safe side-by-side migration.

## Background
**Depends on:** 018, and results from 004 and 005. Design: `docs/architecture.md`, sections Consistency, Targets.

## Approach
- `Embedder` and `VectorIndex` traits with deterministic fakes, plus edge implementations. `VectorIndex` follows the Backends
  section and has a reusable contract suite that passes on the fake and the real index.
- An embedding store keyed by content hash and model, so identical text is embedded once.
- Text is embedded with a deterministic context prefix: document title and heading path.
- A background job with a CPU cap and batching.
- The trait exposes the watermark and generation flip. With pgvector, the watermark is a table row and a generation is a table
  per model. An embedded index, if spec 004 chooses one, uses an atomically renamed manifest and generation directories. A second model builds a parallel
  generation and flips only when complete.

## Verification
1. A rebuild makes 0 embedder calls, proven with a counting fake and on the real store.
2. Kill harness for 200 cycles: the watermark is never ahead of the log, and the index catches up.
3. On the real corpus, recall@50 and p95 stay within 5% of the spec 004 result.
4. A model migration keeps serving throughout, and queries never mix vectors from two models.
5. Identical text embeds once. Storage per segment is measured and recorded.
6. Bad cases: a missing model, a wrong dimension (refused at startup), and an embedding failure that retries one job only.

## Out of scope
Fusion and reranking (020), multi-vector or visual embeddings, and LLM-written context prefixes.

## Result
Not started.
