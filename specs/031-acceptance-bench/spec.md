---
id: 031
name: acceptance-bench
description: Run the whole backend end to end at the test scale and measure every target
---

# 031 acceptance-bench

## Goal
Prove the backend meets the requirements as a whole. Every target in `docs/architecture.md` is measured at 200k segments and projected to 1M, and
one scripted run exercises the full path from upload to audit.

## Background
**Depends on:** all of 002-030 except the dropped 010, plus 032. Design: `docs/architecture.md`, sections Requirements, Targets.

## Approach
- A benchmark harness in the repo with a reproducible corpus generator built on public text, and scripts that take the segment count as a parameter, so spec 033 reuses them.
- An end-to-end scenario through the public APIs: upload a digital PDF, a scanned PDF and a recording. Extract, review and commit.
  Create a page and a new entity type. Search by text, identifier and type. Follow a result to a highlighted region and a time
  range. Run an audit. Then erase a source.
- Measure all target metrics, including during ingest, and cold and warm.
- A requirements checklist with evidence for each item, using the default build, with Postgres as the only external
  process. Verify using `lsof` and `otool -L`.

## Verification
1. A results table: target, measured value, pass or fail, for every row of the Targets section.
2. The scenario passes end to end and can be re-run with one command.
3. Requirement checklist complete, each with a link to a test or measurement.
4. With the LLM endpoint down: search, reading, upload and OCR still work, and extraction waits. With Postgres down: requests
   return 503, no acknowledged data is lost, and service recovers without a restart.
5. `otool -L` on the release binary lists system libraries only, and `lsof` shows no connection except Postgres and the configured
   LLM endpoint.
6. Any failed target becomes a follow-up spec, or a target revision with a recorded reason and the user's OK.

## Out of scope
New features, load beyond one machine, and a security audit by a third party.

## Result
Not started.
