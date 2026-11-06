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
Built durable jobs and upload. `Table::Job` rows hold state, attempts, the lease and a dedupe key. `memex_core::jobs` holds the pure rules (claim, succeed, fail, backoff, retry, cancel), and `Server` has `upload`, `claim_job`, `complete_job`, `fail_job`, `retry_job` and `cancel_job`, plus source and job reads. `workers.rs` in the binary is the pool, on threads separate from request handling, with a limit per kind. A panic in a handler counts as that job's failure. Routes: `POST`/`GET sources`, `sources/{id}`, `sources/{id}/versions`, `upload-limits`, `jobs` with kind and state filters (`state=dead` is the dead-letter list), `jobs/{id}` with attempts and errors, `retry` and `cancel`. Trust-tier and metadata changes are proposals (`set_source_trust`, `set_source_metadata`), and raising an untrusted source goes to review.

**What makes results exact.** A job's results and its completion commit in one transaction, and a worker whose lease expired is fenced: its attempt count no longer matches, so its late commit writes nothing.

**Verification.** `cargo test -p memex-core --test jobs` and `memex-http` and the worker tests pass, with no database, plus the harness on Postgres.
1. Kill harness on Postgres: 200 `kill -9` cycles of a 4-worker pool over 1,500 jobs, where each result has a fresh random id so a second run would really show, and 1 in 10 attempts fails: **1,500 done, 0 dead, 0 open, 1,500 results, 0 duplicates, 0 results without a done job, 0 done jobs without a result.** 339 jobs took more than one attempt, and 165 cycles stopped part way, so the kills did land during work. Restarted workers use a shifted clock, so old leases count as expired with no waiting.
2. An always-failing job goes dead after 5 attempts, all with their errors, while another job finishes. A person can retry it, and read scope can't.
3. Expired leases are recovered, tested with `ManualClock` and no sleeping, including the old worker being refused. A job that keeps crashing its worker goes dead.
4. Bad cases: oversized (413), empty, unsupported bytes (415), a disconnect mid-upload, and bad trust tiers create nothing and leave no blob. Wrong scope is refused before any byte is read.
5. The same bytes twice give one blob, one Source and one job, with 200 and `existing: true` the second time.
6. A saturated queue gives 429 `busy` without reading the body, and finishing work makes room. The upload itself streams through a bounded channel, so a slow disk slows the client.
7. Lists at 10,000 rows filter and page correctly while jobs change state: 20 pages, each job exactly once, with done and queued counts right.

**Decisions.**
- Each upload's Source and job commit in one `state.recorded` event. Jobs are in the log too, which is about four events per job. That's a small cost for making the result and the completion atomic.
- A file's type comes from its bytes. Only the name separates Markdown from plain text.
- `source.recorded` went to version 3 with an upcaster, and the golden hash changed because Source rows gained `name`, `size`, `logical_id`, `version_no` and `metadata`.
- Upload limits are constants for now (2 GiB, 10,000 open jobs), published at `upload-limits`. Making them settings is a small change when someone needs it.

**Not covered:** audio types wait for spec 028. HTTP-level disconnect goes through the same `Read` error path, tested at the core level rather than with a real dropped socket.
