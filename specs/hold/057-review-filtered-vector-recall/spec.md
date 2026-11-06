---
id: 057
name: review-filtered-vector-recall
description: "Decide whether pgvector's weaker recall for filters matching 6 to 20% of rows is acceptable"
---

# 057 review-filtered-vector-recall

> **On hold (2026-11-06):** Assumption in force, waiting for the user's review: keep pgvector with a revised target for filters matching 6 to 20% of rows

## Goal
Have the user decide whether to accept pgvector's miss for mid-range filters, or to pay for an embedded index that passes.

## Background
**Depends on:** 004, whose Result has every number. Target was recall@50 ≥0.95 and p95 ≤15 ms under all filters at 200k on 8 threads. pgvector misses for filters matching about 6 to 20% of rows: 0.933 at 16.0 ms at 10%, and 0.939 at 13.6 ms at 20%. All other conditions pass. Projected to 1M, 10% may fall to about 0.90 recall.

`usearch` passes at 10% (0.987 at 11.3 ms) and is 3 to 5 times faster unfiltered, but it would be a second durable store beside Postgres. It fails at 1% (57 ms), so it needs an exact path too.

**Assumption in force:** keep pgvector, because vectors share the transaction with everything else. Target for that band is revised to recall ≥0.93 and p95 ≤20 ms. Spec 019 builds on this, and spec 033 measures 1M. Lexical results and RRF also compensate for vector misses in hybrid search, which spec 020's golden set can quantify.

**Measured since, through the real index code (spec 019).** Recall is unchanged. p95 at 8 clients is 12.6 ms with no filter, 16.0 ms at 50%, 18.6 ms at 20%, 20.8 ms at 10%, 22.4 ms at 5%, and about 11 ms at 1 to 2%. So 50% and 5% now miss ≤15 ms too, and 10% misses the revised ≤20 ms by 0.8 ms. Hybrid's own target of ≤50 ms still has room.

## Approach
Ask the user: accept, or switch the default `VectorIndex` to `usearch` with an exact path for small filter sets. Switching stays possible later behind the trait.

## Verification
1. The user's answer is recorded here.
2. `docs/architecture.md` and spec 019 match it.

## Out of scope
Other embedded indexes.

## Result
(Filled in when the spec moves to done or dropped: what was built, decisions, numbers, follow-ups.)
