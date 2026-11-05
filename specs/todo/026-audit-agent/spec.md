---
id: 026
name: audit-agent
description: Add scheduled random audit agents that check committed knowledge and measure their own detection rate
---

# 026 audit-agent

## Goal
Agents that periodically pick random committed knowledge and check it against its sources. The same seed gives the same
sample, and at least 80% of planted errors are caught.

## Background
**Depends on:** 025. Design: `docs/architecture.md`, section Audit agent. This is the user's "agents that sometimes do random
checks" requirement.

## Approach
- A scheduler driven by `Clock`, and sampling as a pure function of seed and candidates, weighted toward recent auto-commits and
  by extractor, type, age and trust.
- Deterministic checks first, with no token cost: quote relocation, link rot, staleness TTLs per predicate, and index
  reconciliation counts and hashes.
- LLM checks after that: isolated entailment of the claim against its quote, and blind re-extraction compared with the claim.
- A token budget per run and per day.
- Audit results stored as events. Agents hold `propose` scope, so findings become review items, never direct changes.
- Failures revoke auto-commit for that combination and requeue sibling claims.
- A planted-error mode, with tagged errors in a test vault, measures auditor recall.

## Verification
1. The same seed and state gives an identical sample, proven by a test. A different seed differs.
2. Across planted errors of each kind, at least 100 in total: ≥80% caught overall, reported per kind. False-positive rate
   recorded.
3. An audit failure revokes auto-commit and requeues siblings, end to end with fakes.
4. The budget is enforced, and exhausting it stops the run cleanly and records that.
5. An audit agent's attempt to write directly is refused by scope.
6. Audits don't push search p95 above 1.5× idle.
7. Scheduler tests use `ManualClock` and no sleeping. Bad cases: an empty vault, an LLM outage (deterministic checks still run),
   and a missed schedule.

## Out of scope
Auto-fixing findings, agent reputation and user-facing dashboards.

## Result
Not started.
