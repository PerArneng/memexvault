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
- Routes for run list and detail with seed and sample, on-demand trigger, findings with check results, schedule read and write, budget state, and the planted-error recall report.

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
8. A run triggered through the route with a given seed gives the same sample as a scheduled run with that seed.

**Pass rule for item 6, set before measuring.** At spec 004's 200k-segment corpus with one claim per 20 segments, 4 search clients run hybrid searches with caches off, in six pairs of 30-second rounds, each pair one idle round followed straight away by one round with audits running end to end in a loop (a sample of 200, on deterministic checks and a reconciliation, with no LLM, as a real model's calls wait on the network and cost this machine nothing). The result passes if the median across pairs of audit p95 divided by idle p95 is at most 1.5. Each pair's two p95s, the ratio and the load average are recorded. A miss stays a miss. If the corpus can't be loaded in reasonable time on the busy machine, the same rule runs at 50k and the result says it's not the 200k number.

## Out of scope
Auto-fixing findings, and agent reputation.

## Result
Not started.
