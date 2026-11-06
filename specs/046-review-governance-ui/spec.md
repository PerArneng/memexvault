---
id: 046
name: review-governance-ui
description: Build the review queue, ChangeSet history, revert and auto-commit status
---

# 046 review-governance-ui

## Goal
A reviewer can clear a queue quickly by keyboard, seeing each claim next to its highlighted source, and can inspect or revert any change.

## Background
**Depends on:** 025, 042. Design: `docs/architecture.md`, section Governance.

## Approach
- A queue with filters and counts, a side-by-side view of the proposal with its source region, the reviewer agent's verdict and validator results.
- Approve, reject with reason, edit then approve, and batches. Keyboard shortcuts for all of them.
- ChangeSet list and detail with ops, author and rationale, inverse preview, and revert.
- An open contradictions list with both claims and their sources.
- Auto-commit table showing per-combination precision, Wilson bound, state and revocation history, with a manual disable. Plain language explains why something is on or off.
- Live queue counts from the activity stream.

## Verification
1. A reviewer can process 20 fixture items using only the keyboard, tested in Playwright.
2. Approve, reject and edit-then-approve each record the signed-in principal and reason, shown through the API.
3. A revert from the UI restores the prior state, shown by the store's dump comparison on a fixture.
4. Items that always need a human never show an automatic path, and an agent token can't approve through a session.
5. Two reviewers on one item: the second gets a clear conflict, and no double decision.
6. Hostile proposal text and rationale render as plain text.
7. Pure queue and shortcut logic has unit tests for happy and bad cases.

## Out of scope
Consensus across reviewers, reputation scoring and notifications outside the console.

## Result
Not started.
