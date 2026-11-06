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
Built, and kept. **One check failed as written:** recall matches spec 004 at every filter, but p95 is 30 to 110% above it, not within 5%. I kept it because hybrid's target of ≤50 ms still has room, and spec 057 now carries these numbers for the user's review.

**What was built**
- `VectorIndex` and `Embedder` in `memex-core`, with `MemVector` and `FakeEmbedder` as fakes and one contract suite for the fake and for pgvector.
- `PgVectors` in `memex-vector`: per-generation tables `vectors_gen<n>`, with the watermark in `vector_generations`, committed in the same transaction as the vectors.
- `OnnxEmbedder` in `memex-inference`: e5-small with `query: ` and `passage: ` prefixes, mean pooling, normalization and truncation at 256 tokens.
- An embedding store in Postgres, keyed by model and content hash. Embedded text is the source's name, the heading and the segment text. Claims embed as their keyword document, with no title.
- `VectorProjector`, which runs on its own thread, and `embed` jobs of up to 32 objects, with one worker and `MEMEX_EMBED_THREADS` (default 4) as the CPU cap.
- `GET /api/v1/vaults/{id}/search/vector`, and `memexvault models fetch` for the model.

**How a change reaches the index.** The projector finds objects with no stored embedding, removes their old vector and queues a job before its watermark moves. The job embeds each object's current text and stores the result. Its completion is a log event, and the projector then writes the vector. So nothing bulky enters the log, and a failure retries one job only.

**Measurements**, from `bench-200k.txt`, `kill-200.txt` and `real-model.txt` here, on spec 004's 200,000 vectors and ground truth. 8 clients, second pass, through the real index code with no embedding:

| Filter matches | Plan | Recall@50 (spec 004) | p50 | p95 (spec 004) |
|---|---|---|---|---|
| All | Graph | 0.974 (0.974) | 8.6 ms | 12.6 ms (9.6) |
| 50% | Graph | 0.957 (0.957) | 9.6 ms | 16.0 ms (9.9) |
| 20% | Graph | 0.939 (0.939) | 11.6 ms | 18.6 ms (13.6) |
| 10% | Graph | 0.933 (0.933) | 14.8 ms | 20.8 ms (16.0) |
| 5% | Exact | 0.999 (0.999) | 16.9 ms | 22.4 ms (11.4) |
| 2% | Exact | 0.999 (0.999) | 7.3 ms | 11.3 ms (6.0) |
| 1% | Exact | 0.999 (0.999) | 5.6 ms | 10.9 ms (5.2) |

Against the targets in force: no filter and 1 to 2% pass ≤15 ms. 50% (16.0 ms) and 5% (22.4 ms) miss ≤15 ms. 20% passes the revised ≤20 ms, and 10% misses it by 0.8 ms. Runs varied by a few milliseconds.

The rest of the gap is that the spike sent one query on a held connection with session settings. A real search also looks up the live generation, filters a `text[]` through a GIN index instead of a small integer through a B-tree, and scopes its settings to one transaction. Scratch-table timing put the GIN and B-tree lookups about level once the pending list was gone, so the exact share of each isn't measured.

| Check | Result |
|---|---|
| Rebuild, no embedding | 0 embedder calls on the fake and on the real store. 95 s at 200k, including the graph. |
| Search during that rebuild | 10,811 searches, 0 failed, p95 12.5 ms, max 38 ms (1 client) |
| 200 kills | 0 ahead of the log, 0 missing, 20,529 events to replay, 162 kills left more than the last event behind. Afterwards 34,000 documents against 34,000 rows, 0 retries. |
| Identical text | Embeds once. 168 of the 200,000 passages shared an embedding. |
| Storage | 3,201 bytes per segment: 212 MB embedding store and 428 MB live generation. A rebuild briefly doubles the generation. |
| Real model | Cosine 1.00000 against spec 004's query vectors on 200 queries. Loads in 1.5 s. |
| Query embedding p95 | 17.7 ms alone, 17.0 ms during background embedding, 37.6 ms with 4 clients on 2 sessions |
| Background embedding | 25 passages/s on 4 threads, so about 2.2 hours for 200k and 11 hours for 1M on this machine |
| Memory | 1.17 GB resident with 3 sessions |
| Whole binary | Upload to vector hits in 16 s, on a fixture PDF. A blank query returns 400. |
| Model move | On pgvector: a search thread ran through it, every answer came from one model, and at most one saw the flip and would retry. On the fakes: both generations follow the log, and it flips only when complete. |
| Bad cases | A missing model and a changed dimension refuse startup. A wrong-length vector kills its job and is never stored. A failed embedding retries that job only. |

**Bugs that measuring found**
- A query vector sent as a text parameter was parsed once per row, which took the 5% filter to 97 ms. It's now a constant in one batched statement, which also cut round trips.
- GIN's pending list cost 14 ms per filtered query. Filter indexes are now built at the flip, with `fastupdate = off`.
- Left free, the planner's exact plan varied with its statistics sample. It's now pinned to the index with no parallel workers.
- The score was formatted as text for every row before sorting, about 3 ms on the scratch table.

**Decisions**
- Searches send literals rather than parameters, since only a simple-query batch gives one round trip. `quote` is the only way user input enters that SQL, with unit tests and a hostile value in the contract suite.
- One embedding worker, which is also what guarantees identical text embeds once. More workers could embed the same text twice at the same moment, which wastes work and harms nothing.
- Passages get their own ONNX session, so a batch never holds up a question. That costs memory.
- A first build or a rebuild flips at once and fills in as jobs finish. A move to a new model waits until complete, since the old model still answers well.

**Known limits**
- A dead embed job leaves its objects without a vector, though keyword search still finds them. It also stops a model move from flipping until someone retries the job. The move logs nothing about this yet, and the live model keeps serving.
- Flipping holds the vector projector thread during the graph build, so every vault's vector index waits. I didn't time the graph alone, but the whole 200k rebuild took 95 s. Keyword search has its own thread.
- A new vault logs one harmless warning from each projector, since it's registered just before its schema exists.
- Migrations now need the `vector` extension. `CREATE EXTENSION` needs a privileged role unless an admin has already created it.
- The manifest offers one model, so a real move between two model files hasn't run. The move was tested with fake models on real pgvector.
- `models fetch` was checked only so far as the URLs answering with the expected size. I used the spike's copies, whose hashes match.

**Postgres suite:** 8 of the `pg_` tests pass with this work, plus 3 `pg_vector_` tests. Four stop before testing anything, as they need what a plain run doesn't provide: `pg_child` and `pg_job_child` are helpers, `pg_kill_postgres_100_times` needs `MEMEX_TEST_CONTAINER`, and the full-disk test needs a Postgres on a small disk. None of those four was run here.

**Follow-ups:** spec 021 caches query embeddings. Spec 030 must erase and compact embeddings and vector tables, and now says so. Spec 033 measures 1M. Spec 057 has the new numbers.
