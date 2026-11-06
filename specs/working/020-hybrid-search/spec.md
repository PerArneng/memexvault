---
id: 020
name: hybrid-search
description: Combine lexical, vector and rerank signals into a fast, citation-bearing evidence package
---

# 020 hybrid-search

## Goal
One search API that returns a token-budgeted evidence package with citations. Pass: recall@20 ≥95% on a 200-question
golden set, p95 ≤50 ms for hybrid search and ≤300 ms with reranking at the test scale.

## Background
**Depends on:** 019, and the reranker from 005. Design: `docs/architecture.md`, sections Search, Targets.

There is no LLM on the read path. The calling agent does the generating.

## Approach
- A pure rule-based planner: exact lookup, identifier-heavy, natural-language, or filtered.
- Parallel candidate generation, then Reciprocal Rank Fusion (default k=60), then opt-in reranking of the top 20 at 128 tokens (spec 005, assumption under review in 055).
- Typed filters: kind, type, source, trust tier, epistemic status, valid time.
- Fence all hits against store status with one batched lookup. Support `as_of` and `min_seq`.
- An evidence package with claims first, then segments, then parent sections. Each item has anchors, epistemic status, trust
  tier and conflicts. It also has token budget and cursor handling, and says explicitly when nothing relevant was found.
- A golden set of 200 questions over a public corpus committed as fixtures, labelled by category and language. At least 40 per
  language (English, Swedish, German, Spanish) and 20 where the question and answer differ in language.
- Lexical-only and vector-only modes for comparison and debugging, per-signal scores and the planner's path in an opt-in debug field, and opt-in facets with counts for the typed filters.

## Verification
1. Recall@20 ≥95% overall and ≥90% per language, plus nDCG@10 and MRR, compared against lexical-only and vector-only, per category
   and language. Cross-language recall and compound-word misses are reported on their own.
2. Latency p50, p95 and p99 at 200k segments with 8 concurrent clients meets the targets, with a projection to 1M.
3. Every returned item resolves to an anchor or is explicitly marked as having none.
4. Retracted objects are never returned, tested by retracting between candidate generation and fencing with fakes.
5. Unanswerable golden questions return a low-relevance flag at least 80% of the time.
6. Bad cases: empty query, huge query, budget too small to fit one item, `as_of` before the first event, and `min_seq` that
   can't be reached in time.
7. Fusion, planner and budget logic are unit-tested in memory.
8. Store round trips per search don't grow with the number of hits: the counting fake reports the same count for 10 and for
   200 hits.
9. Requesting facets or debug data leaves the default response and its latency unchanged, measured.

## Out of scope
Query caching (021), entity and page results (added by 022 and 023), and LLM query rewriting.

## Result
Built, and recall passes. **Verification 2 fails as written:** with 8 concurrent clients, hybrid search is p95 81 ms against ≤50 ms, and rerank is 1,030 ms against ≤300 ms. With 1 client both pass, at 31 ms and 229 ms. Embedding the query takes most of that time, so this waits on the user's decision, below.

**What was built**
- `GET /api/v1/vaults/{id}/search` and `Server::search`, on `memex_core::search`: `plan`, `fusion`, `budget`, `relevance`, `assemble`, `history` and `run`.
- `Reranker`, with `FakeReranker` and `OnnxReranker`. `models fetch` and `models check` now cover the reranker.
- Type and trust tier in both indexes. Their filters, and two new batched reads, `edges_touching` and `sections_of`, are in the contract suites.
- Vault migration 6, which drops vector generations so they rebuild from stored embeddings. The keyword index moved to schema 2, so it rebuilds too.
- 230 golden entries in `crates/memexvault/tests/fixtures/golden/golden.jsonl`, sha256 `d3f4d030…`, with `tools/golden_tool.py`. An agent that never saw the search code wrote them, from a seeded sample, before the first measurement. The file hasn't changed since.

**Results**, from `golden-200k.txt`, `latency-50k-200k.txt` and `latency-stages.txt` here, on spec 004's 200,000 Wikipedia paragraphs.

| # | Check | Target | Measured | |
|---|---|---|---|---|
| 1 | Recall@20 overall | ≥95% | 97.5% (195/200) | pass |
| 1 | Recall@20 per language | ≥90% | en 98.2, sv 98.0, de 97.9, es 95.8 | pass |
| 2 | Hybrid p95, 8 clients | ≤50 ms | **81.2 ms** (p50 52.9, p99 97.1) | **fail** |
| 2 | Hybrid p95, 1 client | ≤50 ms | 31.0 ms (p50 23.7, p99 34.4) | pass |
| 2 | Hybrid with rerank p95, 8 clients | ≤300 ms | **1,030 ms** (p50 856, p99 1,088) | **fail** |
| 2 | Hybrid with rerank p95, 1 client | ≤300 ms | 229 ms (p50 209, p99 255) | pass |
| 3 | Every item has an anchor or says it has none | all | Tested for located, broken and none. Over HTTP, a segment's offsets lead to its exact text. | pass |
| 4 | Retracted between candidates and fence | never returned | Tested with a fake index that retracts during search, for a segment and a claim | pass |
| 5 | Low-relevance flag on unanswerable questions | ≥80% | 83% (25/30), and 12/14 on the half not used to choose the thresholds | pass |
| 6 | Bad cases | handled | All five tested, and tried on the real binary except a `min_seq` that times out, which is an HTTP test on fakes | pass |
| 7 | Fusion, planner and budget tested in memory | yes | Yes | pass |
| 8 | Store reads, 10 hits against 200 | same | Equal, at most 10, and the same with facets and debug | pass |
| 9 | Facets and debug leave the default alone | unchanged | The default response is identical. Default p95 78.6 to 80.1 ms beside them. They cost about 6 ms more only when asked for. | pass |

The low-relevance flag also fires on 7.5% of answerable questions (15/200), so a caller shouldn't treat it as proof that nothing is there.

| Mode | Recall@20 | nDCG@10 | MRR | Cross-language recall | Compound recall |
|---|---|---|---|---|---|
| Hybrid | 97.5% | 0.850 | 0.820 | 85% (17/20) | 100% |
| Keyword only | 91.0% | 0.801 | 0.769 | 90% | 81% |
| Vector only | 92.0% | 0.852 | 0.833 | | |
| Hybrid with rerank | 97.5% | 0.940 | 0.929 | | |
| Hybrid inside a 10% source filter | 99.5% | 0.929 | 0.917 | | |

Per-category and vector and rerank cross-language figures are in `golden-200k.txt`. An earlier index of the same data scored 98.0%, because a graph build isn't deterministic, and a difference of one question is noise. Hybrid's five misses are two heavy paraphrases and three cross-language questions.

**Latency at 8 clients, p95**, with a projection that's an estimate, not a measurement:

| Path | 50k | 200k | 1M, fitted | 1M, linear worst case |
|---|---|---|---|---|
| Hybrid | 80.9 | 81.2 | 82 | 406 |
| Keyword only | 13.5 | 21.3 | 36 | 106 |
| Vector only | 80.2 | 79.4 | 78 | 397 |
| Hybrid, 10% source filter | 76.4 | 87.5 | 102 | 437 |
| Hybrid with rerank | 921 | 1,030 | 1,173 | 5,150 |
| Exact id | 8.9 | 13.1 | 21 | 66 |

The load average was 8 to 19 during these runs, from other work on the machine, with about 70% of the CPU idle in a sample taken just before an earlier run. Docker's port forwarding adds to every store read: a snapshot and one read cost 2.3 ms at p50.

**Where the time goes.** Under 8 clients, alone: embedding p50 28, p95 62 ms; vector index p50 10, p95 16 ms; a keyword-only search with its fence p50 14, p95 23 ms. So embedding alone is over budget, and more sessions don't fix it:

| Query sessions × threads | Hybrid p95, 8 clients | Hybrid p95, 1 client | Memory |
|---|---|---|---|
| 2 × 4 (default) | 92 ms | 31 ms | 484 MB |
| 4 × 2 | 81 ms | 38 ms | 679 MB |
| 8 × 1 | 80 ms | 52 ms | 1,049 MB |

These three were one earlier run each, on a busier machine.

**One option tried, not adopted:** embedding queries with spec 005's int8 model against the same f32 passage vectors gives hybrid p95 52.1 ms at 8 clients and 25.5 ms at 1. In the run on the earlier code, recall was 97.5% with cross-language at 90% and 20/200 wrong flags, against 15/200. That's close to the target but not under it, and the int8 file is our own quantization, which `models fetch` has no pinned source for.

**Other answers specs asked for**
- **Decompounding (018) isn't needed.** Hybrid finds all 16 compound questions. Keyword search alone misses 3, and vectors make up for them.
- **The common-term rule (018) costs no recall.** Without it, hybrid is 96.0% and keyword search 89.0%, both slightly lower than with it.
- **Reranking (055) doesn't change recall, but it changes rank a lot:** nDCG@10 0.850 to 0.940, and it brings the right passage nearer the top on cross-language questions. It reads 20 candidates at 128 tokens as spec 005 chose. 8 clients at once exceed what the CPU can do in 300 ms.
- **Exact lookups** miss the design's ≤2 ms: p95 4.8 ms at 1 client, 13.1 ms at 8, p50 2.5 and 5.6. This spec's Verification doesn't cover them, but `docs/architecture.md` does. A lookup takes about 5 round trips to a store behind Docker's port forwarding, and I reduced them as far as the current `Store` calls allow.

**Decisions**
- **`as_of` replays the log**, as the design says the store serves it, and scans for keywords with no stemming or vectors. It took 4.9 s at 200k, so it's limited to two at a time, with 429 beyond that. There's no target for it. It takes an event or, as `as_of_ms`, a time, because event numbers start at 0 and only a time can fall before the first event.
- **`type` means a segment's media type or a claim's predicate**, as the spec didn't define it. Spec 023 can add entity types to it.
- **A failed signal degrades** hybrid search and says so, rather than failing it. The design says only that unfenced results are never served.
- **A page fences only what it needs,** plus 10 candidates, unless many have gone. Reading all of them took most of the fence's time.
- **The flag's thresholds, 0.85 cosine and 0.65 word coverage,** are specific to e5-small. Rerank scores aren't used: at a floor catching 80%, they wrongly flag about a quarter of answerable questions.

**Known limits**
- Segment citations carry offsets and pages but no rectangles, since those would need the word table on every search. Spec 042's viewer has to get them from the word table, and no route gives a segment's rectangles yet.
- `min_seq` guarantees the keyword index. New text can still be waiting for its embedding, so vector search may not find it yet.
- Each golden question labels one passage, so a right answer in another passage counts as a miss. The corpus has no titles, and the harness's sources and headings are synthetic, and vectors were embedded without the heading.
- Latency used 10,000 claims in one run only, which added about 1 ms at p95 at 8 clients (82.2 ms). Search during ingest wasn't measured here.
- The golden file holds short answers taken from Wikipedia, which is CC BY-SA.

**A mistake to know about.** My first runs used the shared development database, so `migrate` moved all 18 of its vaults to a schema that older binaries refuse. Those runs also dropped any vector generations in the other 16 vaults, which rebuild from stored embeddings on the next start. Then `pg_bad_cases`, which hard-coded the database's name, failed in my separate database and left `synchronous_commit=off` set on the shared one. I dropped my 2 vaults, put the other 16 back to version 5, reset the setting, and made the test use the URL's database. AGENTS.md now says a branch that adds a migration should use its own database. All later Postgres runs did.

**Bugs measuring found**
- `sections_of` used the wrong JSON path on Postgres, so sections never came back, and the first latency runs were optimistic. The contract suite caught it, and the harness now fails unless sections appear (173 of 200 default requests at 200k).
- Sessions were picked round-robin, so a query could queue behind a busy one while another sat free.

**Other verification.** `cargo test --workspace`: 353 passed, 0 failed, 39 ignored. `cargo clippy`, `cargo fmt --check` and `cargo deny check licenses` pass. `pg_contract`, `pg_bad_cases` and the `pg_vector_` tests pass on the final code. `pg_batched_lookups`, `pg_group_commit_throughput`, `pg_kill_jobs_200_times`, `pg_kill_memexvault_1000_times`, `pg_registry_contract` and `pg_replay_of_100k_events_matches_mem_store` passed in a run just before the `sections_of` fix, which they don't exercise. The keyword and vector kill harnesses from specs 018 and 019 weren't run again. `pg_child`, `pg_job_child`, `pg_kill_postgres_100_times` and the full-disk test need what a plain run doesn't provide, as in spec 019, and weren't run. The real binary, driven over HTTP, did upload to search in about 2 s on a small text file, with rerank on the real model.

**Follow-ups:** the decision on concurrent latency. Spec 021 caches query embeddings, which helps repeated questions only. Spec 042 needs segment rectangles. Spec 033 measures 1M. Spec 055 has new numbers.
