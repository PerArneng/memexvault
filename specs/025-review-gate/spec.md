---
id: 025
name: review-gate
description: Add the reviewer agent and human review queue, with auto-commit earned only from audited precision
---

# 025 review-gate

## Goal
Extracted knowledge is reviewed before it becomes canonical. Injection fixtures cause 0 auto-commits and 0 tool calls, and
auto-commit is possible only where audited precision has earned it.

## Background
**Depends on:** 024. Design: `docs/architecture.md`, sections Governance, Prompt injection.

## Approach
- A reviewer agent sees only the claim and its quote, and returns a verdict and reason. It has no tools and `propose` scope
  at most.
- Human queue API with batches, filters, approve, reject and edit-then-approve. Decisions are attributed and stored.
- A precision table per extractor, prompt version and type. The Wilson lower bound is a pure function. Auto-commit turns on per
  combination only when the bound clears a configured threshold, and it's off until then.
- Always-human-reviewed rules from spec 013 can't be overridden.
- Untrusted sources can't reach `verified` automatically.
- Routes for item detail with the reviewer agent's verdict, queue counts, the auto-commit table (precision, Wilson bound, on or off, revocation history, threshold) and a manual disable.

## Verification
1. At least 30 injection fixtures (instructions in text, in hidden text, in file metadata, and in fake tool output): 0
   auto-commits, 0 tool calls, 0 changes to other objects.
2. Reviewer agreement against human labels on a labelled set is recorded, with a precision of ≥0.9 on approvals.
3. Wilson bound unit tests, with exact values including 0 samples and all failures.
4. A combination crossing the threshold turns on auto-commit, and falling below turns it off, both with fake data.
5. Every decision is traceable to who decided, when and why.
6. Bad cases: approving twice is idempotent, approving a stale claim conflicts, and an LLM outage leaves items in the queue.
7. A manual disable stops auto-commit for that combination at once, and only re-earning it can turn it on again.

## Out of scope
Consensus across multiple agents, agent reputation scoring.

## Result
Not started.
