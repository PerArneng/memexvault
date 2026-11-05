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
- Parallel candidate generation, then Reciprocal Rank Fusion (default k=60), then opt-in reranking of the top 50.
- Typed filters: kind, type, source, trust tier, epistemic status, valid time.
- Fence all hits against store status with one batched lookup. Support `as_of` and `min_seq`.
- An evidence package with claims first, then segments, then parent sections. Each item has anchors, epistemic status, trust
  tier and conflicts. It also has token budget and cursor handling, and says explicitly when nothing relevant was found.
- A golden set of 200 questions over a public corpus committed as fixtures, labelled by category and language. At least 40 per
  language (English, Swedish, German, Spanish) and 20 where the question and answer differ in language.

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

## Out of scope
Query caching (021), entity and page results (added by 022 and 023), and LLM query rewriting.

## Result
Not started.
