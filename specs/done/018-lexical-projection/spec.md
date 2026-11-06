---
id: 018
name: lexical-projection
description: Project segments and claims into a tantivy index that can be rebuilt from the log
---

# 018 lexical-projection

## Goal
A lexical index fed only by a projector. A rebuild and an incremental run yield the same hits, indexing lag has p95 ≤1 s,
and search p95 is ≤15 ms at the test scale.

## Background
**Depends on:** 015, and results from 006. Design: `docs/architecture.md`, sections Consistency, Languages, Search, Targets.

## Approach
- `LexicalIndex` trait as the Backends section defines it, with a fake, a reusable contract suite and `memex-lexical` on
  tantivy. No tantivy type or query syntax crosses the trait.
- A schema for segments and claims, extendable for pages (022) and entities (023). Language-neutral, diacritic-folded and
  per-language stemmed fields as the Languages section defines, so part numbers and identifiers match literally. Status and
  time as fast fields.
- An idempotent projector keyed by object id. The trait exposes the watermark, and tantivy keeps it in the commit payload.
  Startup catch-up.
- Rebuild to a new generation directory, then flip a pointer, keeping the old one for rollback.
- A commit-interval policy as a pure function, trading off lag against merge cost.

## Verification
1. Incremental and rebuilt indexes return the same hit sets across 1,000 test queries on a generated corpus.
2. Kill harness for 200 cycles: the index is never ahead of the log and always catches up.
3. Lag p95 ≤1 s under sustained ingest, and search p95 ≤15 ms at 200k segments, with a projection to 1M.
4. Rebuild time at 200k segments ≤3 minutes, while the old generation keeps serving.
5. Identifier queries such as `ORA-00904` and `XK-42/B` match exactly.
6. Bad cases: a retracted claim stops appearing; a corrupted index generation is detected and rebuilt; query syntax errors
   return a clear error instead of a 500.
7. The contract suite passes on the fake and on tantivy.
8. Language tests: a Swedish, German and Spanish inflected form each finds its base form. `Goteborg` finds `Göteborg`, and `Göteborg`
   ranks exact matches first. Search p95 stays within the target with all fields queried.

## Out of scope
Vector search (019), fusion (020), decompounding unless spec 020 shows a need, and non-Latin scripts.

## Result
Built. Keep tantivy: every target at 200k is met except one secondary ratio, noted below. Whole-sentence queries are likely to
miss 15 ms at 1M, and spec 033 has to measure that.

**What was built**
- `LexicalIndex` in `memex-core`, with `MemLexical` as the fake, one contract suite for both, and `TantivyLexical` in `memex-lexical`.
- `LexicalProjector`, which re-reads the current rows an event touched, so replay is idempotent. A rebuild pages through tables with
  a fresh snapshot per page, catches up from the head it started at, and then flips.
- Generations in `gen-<n>/`, with an atomically renamed `CURRENT` pointer. Live and previous stay, and others are swept.
- `GET /api/v1/vaults/{id}/search/lexical`, and `workers::start_projector` in the binary.
- `tantivy` 0.27 with default features off, so there is no zstd C library.

**Measurements**, from `bench-200k.txt` and `kill-200.txt` in this folder, on 200,000 real paragraphs in 4 languages:

| Check | Target | Measured |
|---|---|---|
| Lag from commit to searchable, 952 segments/s with edits and deletes | p95 ≤1 s | p95 519 ms, max 586 ms |
| Keyword search, 8 clients, limit 100 | p95 ≤15 ms | 1.20 ms |
| 10-word question, 8 clients, limit 100 | p95 ≤15 ms | 4.87 ms |
| 32-word sentence, 8 clients, limit 100 | p95 ≤15 ms | 12.10 ms (p99 16.77 ms) |
| Identifier, 8 clients, limit 100 | p95 ≤15 ms | 0.09 ms |
| Keyword search during ingest | p95 ≤15 ms | 3.30 ms |
| Rebuild | ≤3 min | 7.5 s, 0 failed searches, old generation p95 1.47 ms (max 61.6 ms) |
| Incremental against rebuilt hit sets | same | 0 of 1,000 keyword queries differ (1,506,380 hits), 0 of 1,000 sentences differ |
| 200 kills | never ahead, always catches up | 0 ahead, 0 corrupt, 0 unusable, 0 missing, 459 events replayed; 34,000 documents against 34,000 rows afterwards |

Disk was 151 MB following the log and 310 MB with both generations. Reopening took 3.3 ms and a deep check 15 ms.

**Projection to 1M.** This is an estimate, not a measurement. p95 at 8 clients and limit 100 for sentences, at 1 client for the others:

| Query | 50k | 200k | 1M, fitted to the growth seen | 1M, linear worst case |
|---|---|---|---|---|
| Keyword | 0.79 ms | 1.00 ms | about 1.3 ms | 5 ms |
| 10-word question | 2.04 ms | 3.87 ms | about 8 ms | 19 ms |
| 32-word sentence | 6.09 ms | 12.10 ms | about 27 ms | 60 ms |

So keywords should hold, questions probably hold, and whole sentences probably miss 15 ms at 1M. Spec 033 decides this. Spec 020 can
also limit the terms of very long queries.

**Decisions and deviations**
- **Common-term rule, added to meet the target.** Before it, sentence queries were about 20 ms p95 at 50k. With at least 10,000
  documents, single-term clauses matching over 4% of documents are dropped, unless every clause is common. Spec 020's golden set has
  to check what this costs in recall.
- **No status field,** which differs from the Approach. Only current rows are indexed: Active, not superseded, and on the current
  rendition. So `as_of` in spec 020 can't find retracted rows through this index and needs another route.
- **`CommitPolicy`:** every 500 ms while busy, 30 s when idle, or at 50,000 pending operations.
- **Query language:** words and double-quoted phrases only, which is why there are no engine syntax errors to leak. Oversized
  queries return a 400.

**Known limits**
- Search during ingest is 3.3 times idle (3.30 ms against 1.00 ms). The design asks for at most 1.5 times. It stays far under
  15 ms, so I left it, but the ratio isn't met.
- `Mueller` doesn't find `Müller`, because folding gives `muller`. `C++` is searched as `c`. `XK-42/B` matches as a phrase of its
  parts, so the text `XK-42 B` matches too.
- The common-term rule uses each index's own document frequencies, so a term at the 4% boundary could differ between incremental
  and rebuilt indexes. None did in 2,000 queries. Top-20 overlap was 99.49%, because BM25 statistics include deleted documents.
- Evidence rows are never deleted on their own, so the projector doesn't handle that case.
- `drop_vault` waits with no lock timeout for any open snapshot of that vault. Snapshots are short-lived in production, but one
  stuck reader would block a vault delete. Spec 030 should set a timeout.

**The first kill run was invalid.** It reported 0 corrupt, but no index existed yet: a first build outlasts the longest wait before a
kill, and a missing index was counted as fine. It then hung on its own snapshot lock. The harness now builds first, and fails if an
index is missing or if no kill left anything to replay. The results above are from the fixed harness.

**Other verification:** the contract suite on the fake and on tantivy, language tests, identifier tests, retraction, corruption
detection with rebuild, and bad queries returning 400 all pass in `crates/memex-core/tests/lexical.rs` and
`crates/memexvault/tests/lexical.rs`.

**Follow-ups:** spec 020 handles `as_of` for retracted rows and the recall check. Spec 033 measures at 1M. Spec 030 adds the timeout.
