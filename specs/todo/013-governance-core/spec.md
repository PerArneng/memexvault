---
id: 013
name: governance-core
description: Build the proposal flow with validators, risk decisions, review queue, idempotency and revert
---

# 013 governance-core

## Goal
Every knowledge write goes through one path: proposal, validators, risk decision, commit or review. Retries commit once,
and any commit can be reverted.

## Background
**Depends on:** 012. Design: `docs/architecture.md`, sections Data model, Governance.

## Approach
- ChangeSet with ops, rationale, evidence, idempotency key, and base versions. Assertion and Evidence objects with
  epistemic status and bi-temporal fields.
- A validator pipeline as pure functions: schema, scope, evidence required for machine claims, quote check, and dedup and
  contradiction using a canonical hash.
- A pure risk function returning auto-commit, review or reject. Always-review rules are hard-coded in one table and tested.
  Machine auto-commit is off by default, and only spec 025 can earn it.
- Review queue tables and API: list, approve, reject, with reason and reviewer attribution.
- Conflict responses for stale base versions, returning current state. A computed inverse and revert.
- A changes-since-cursor API.
- Routes for proposal status with validator results and rejection reasons, dry-run validation, ChangeSet list and detail with ops, inverse preview and a reversibility check, revert, decision history, and a list of open contradictions.

## Verification
1. Sending the same idempotency key 100 times, concurrently, commits once and returns the same result.
2. A random ChangeSet followed by its revert leaves the materialized state unchanged across 10k property runs. Cases that
   can't be reverted are refused up front.
3. Each always-review kind is shown to never auto-commit, even at confidence 1.0.
4. An unsourced machine claim can't become active. A contradictory claim coexists with a link to the other claim.
5. Stale base versions return a structured conflict. No partial ChangeSet is ever visible to a concurrent reader.
6. `as_of` on transaction time returns what was believed at a past sequence.
7. A dry run writes nothing, and its validator results match a real submission of the same proposal.

## Out of scope
LLM agents (specs 024-026), pages (022), entity types (023).

## Result
Not started.
