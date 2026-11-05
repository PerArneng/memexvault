---
id: 018
name: lexical-projection
description: Project segments and claims into a tantivy index that can be rebuilt from the log
---

# 018 lexical-projection

## Goal
A lexical index fed only by a projector. A rebuild and an incremental run yield the same hits, indexing lag has p95 ≤1 s,
and search p95 is ≤15 ms at 1M segments.

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
3. Lag p95 ≤1 s under sustained ingest, and search p95 ≤15 ms at 1M segments.
4. Rebuild time at 1M segments ≤15 minutes, while the old generation keeps serving.
5. Identifier queries such as `ORA-00904` and `XK-42/B` match exactly.
6. Bad cases: a retracted claim stops appearing; a corrupted index generation is detected and rebuilt; query syntax errors
   return a clear error instead of a 500.
7. The contract suite passes on the fake and on tantivy.
8. Language tests: a Swedish, German and Spanish inflected form each finds its base form. `Goteborg` finds `Göteborg`, and `Göteborg`
   ranks exact matches first. Search p95 stays within the target with all fields queried.

## Out of scope
Vector search (019), fusion (020), decompounding unless spec 020 shows a need, and non-Latin scripts.

## Result
Not started.
