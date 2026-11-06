---
id: 055
name: review-reranker
description: Decide whether reranking 20 candidates at about 130 ms replaces the rerank-50 target that no local model met
---

# 055 review-reranker

> **On hold (2026-11-06):** Assumption in force, waiting for the user's review: optional rerank of 20 candidates at 128 tokens with mmarco int8

## Goal
Have the user confirm how reranking should work, since no local model could rerank 50 passages within 250 ms on CPU.

## Background
**Depends on:** 005, whose Result has the numbers. The original target was rerank-50 p95 ≤250 ms, with hybrid with rerank-50 ≤300 ms. Measured on the reference machine: `mmarco` int8 at 128 tokens took 287 ms for 50 and 133 ms for 20. `bge-reranker-v2-m3` int8 took 495 ms for just 10.

**Assumption in force:** reranking stays opt-in and uses `mmarco` int8 on the top 20 at 128 tokens. Target: rerank p95 ≤250 ms and hybrid with rerank ≤300 ms. Spec 020 builds it this way. Quality effect measured on small samples: Swedish 0.908→0.935, German about unchanged, Swedish query to English text 0.755→0.830.

**Measured by spec 020, on 200 golden questions at 200k:** nDCG@10 0.850 to 0.940, with recall unchanged at 97.5%. Hybrid with rerank p95 is 229 ms with 1 client and 1,030 ms with 8, so the ≤300 ms target holds only for one at a time. Spec 020 closed with this miss unmet at 8 clients and left it to this review. 8 reranks at once need more CPU than 300 ms allows. Hybrid's own miss at 8 clients is in spec 062.

## Approach
Ask whether to accept this, or instead allow a slower, better reranker with a remote service or GPU behind the same `Reranker` trait. Ask whether the ≤300 ms target applies to one client, or to 8 clients too, and if so how: for example by limiting how many rerank at once, or by accepting a higher number.

## Verification
1. The user's answer is recorded here.
2. If accepted, close it. If changed, update `docs/architecture.md` and spec 020.

## Out of scope
Training a model.

## Result
(Filled in when the spec moves to done or dropped: what was built, decisions, numbers, follow-ups.)
