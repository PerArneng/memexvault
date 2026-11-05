---
id: 014
name: jobs-upload
description: Add a durable job queue and document upload so ingestion survives crashes without duplicates
---

# 014 jobs-upload

## Goal
Documents can be uploaded and processed through durable jobs. A crash at any point resumes the work with no duplicate
result, and the API stays responsive while ingestion runs.

## Background
**Depends on:** 012. Design: `docs/architecture.md`, section Consistency.

## Approach
- A job table behind `Store`: state, attempts, lease with expiry from `Clock`, backoff as a pure function, and dead-letter state.
- Worker pool with per-kind concurrency limits, run separately from request handling.
- Streaming upload API with a size limit, content sniffing, trust tier and metadata. Creates a Source version and enqueues a
  rendition job. Identical bytes reuse the blob and return the existing Source.
- Job status, retry and cancel APIs.
- Jobs are idempotent, keyed on input hash and pipeline version.
- Routes for source list, detail and versions, trust-tier change and metadata edit. Job list with filters by kind and state, dead-letter list, and attempts and error detail per job. A route that publishes upload limits and allowed types.

## Verification
1. Kill harness for 200 cycles during mixed jobs: every job finishes exactly once in effect, with 0 duplicate derived
   objects.
2. An always-failing job reaches dead-letter after its attempts, and doesn't block others.
3. Expired leases are recovered, tested with `ManualClock` and no sleeping.
4. Bad cases: oversized and empty uploads, unsupported types, client disconnect mid-upload (no Source is created), and
   wrong scope.
5. Same bytes uploaded twice produce one blob and one Source version.
6. Queue saturation returns a clear backpressure response rather than unbounded memory growth.
7. Job and source lists filter and page correctly at 10,000 rows, including while jobs change state.

## Out of scope
Parsing itself (spec 015), connectors that pull from other systems, and email or web import.

## Result
Not started.
