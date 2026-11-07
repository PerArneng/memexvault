---
id: 062
name: review-hybrid-latency
description: Decide what to do about hybrid search missing its 50 ms p95 target with 8 concurrent clients
---

# 062 review-hybrid-latency

## Goal
Have the user decide how to treat hybrid search's p95 of 81 ms with 8 concurrent clients, against the ≤50 ms target. With 1 client it's 31 ms, so the miss appears only under concurrent load.

## Background
**Depends on:** 020, whose Result, `latency-50k-200k.txt` and `latency-stages.txt` have the numbers. All are at 200k segments on the reference machine, with a load average of 8 to 19 from other work.

| Hybrid p95 | 1 client | 8 clients | Target |
|---|---|---|---|
| Measured | 31.0 ms | 81.2 ms | ≤50 ms |

- **Cause:** embedding the query. Alone under 8 clients, embedding is p50 28 ms and p95 62 ms, over the whole budget by itself. The vector index is p95 16 ms, and keyword search with its fence is p95 23 ms.
- **More sessions barely help:** 2 sessions × 4 threads gave 92 ms, 4 × 2 gave 81 ms at a 1-client cost of 38 ms, and 8 × 1 gave 80 ms at 52 ms with 1 client and 1,049 MB.
- **Growth:** 80.9 ms at 50k and 81.2 ms at 200k. The projection to 1M is 82 ms fitted and 406 ms linear, and only spec 033 can measure it.
- **Int8 query embedding isn't adopted.** It reached 52.1 ms at 8 clients, which is still over target, and it wrongly flagged 20 of 200 answerable questions against 15. The file is our own quantization, with no pinned source. The user turned it down when closing spec 020.
- **Other targets missed at 8 clients, same run:** exact lookup p95 13.1 ms against ≤2 ms, and 4.8 ms with 1 client. Keyword-only search p95 21.3 ms against ≤15 ms.
- **Entity exact filters (spec 023) miss the same ≤2 ms target:** p95 2.28 to 4.02 ms at 200k values on a shared machine, with a primary-key read alone at 2.36 ms. Spec 023 landed with this unmet, by the user's decision.
- **The rerank miss is in spec 055:** 1,030 ms against ≤300 ms at 8 clients.

**Assumption in force:** spec 020 is closed with this target unmet at 8 clients. `docs/architecture.md` keeps ≤50 ms as the target, and the search stays as built. Spec 021's cache of query embeddings helps repeated questions only.

## Approach
Ask the user which of these to do:
- Say the target applies to one client, or set a different target for 8 clients.
- Keep the target, and open a spec to make query embedding faster under load, such as by limiting how many embed at once or by using a smaller or faster model with a pinned source. A new model has to repeat spec 020's golden measurement.
- Require a quieter machine, and measure again before deciding. The runs in spec 020 shared the machine with other work.

Ask whether the exact lookup and keyword-only misses at 8 clients follow the same decision.

## Verification
1. The user's answers are recorded here.
2. If a target changes, `docs/architecture.md` is updated to match.
3. If the code changes, `golden_recall` and `latency_50k_200k` from `crates/memexvault/tests/golden.rs` are run again, against a database of their own. Recall@20 stays ≥95% overall and ≥90% per language.

## Out of scope
The rerank target, which is spec 055's question. Measuring 1M, which is spec 033's.

## Result
Not started.
