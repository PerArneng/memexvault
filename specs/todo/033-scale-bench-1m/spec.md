---
id: 033
name: scale-bench-1m
description: Measure every target at the full 1M segments once more disk is approved
---

# 033 scale-bench-1m

## Goal
Prove the 1M-segment goal, which earlier specs only project. Every target in `docs/architecture.md` is measured at 1M segments on
the reference machine, and each projection is compared with what actually happened.

## Background
**Depends on:** 031. Design: `docs/architecture.md`, sections Targets, Requirements.

The user set a 20 GB limit on test data for now, so specs before this one measure at 200k segments. Vectors, indexes and Postgres
rows at 1M need more than that, so running this spec needs the user's OK for the extra disk space.

## Approach
- Run the spec 031 harness with the segment count set to 1M.
- Record the disk used before starting, and confirm the space with the user.
- Tune Postgres and index settings if a target misses, recording every change.

## Verification
1. A results table: target, measured value at 1M, the 200k projection, and pass or fail, for every row of the Targets section.
2. Any failed target becomes a follow-up spec, or a target revision with a recorded reason and the user's OK.
3. Total disk use is recorded per component, which also gives operators a sizing guide.

## Out of scope
New features, and scale beyond 1M segments per vault.

## Result
Not started.
