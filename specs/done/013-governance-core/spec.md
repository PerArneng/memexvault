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
Built the one write path for knowledge: `Server::propose` (with dry run), `approve`, `reject`, `revert`, `inverse_preview`, `believed_at`, `open_contradictions`, the pure `governance` module (validators, `risk`, `plan`, `inverse_of`, `check_reversible`, `believed_at`), row types and tables for Segment, Assertion, Evidence and ChangeSet, and the routes under `/api/v1/vaults/{id}/`: `proposals`, `changesets` (list, detail, `inverse`, `approve`, `reject`, `revert`), `changes`, `assertions/as-of` and `contradictions`. All paged routes use the shared list convention, which a test checks in the OpenAPI document.

A commit is one event, `changeset.recorded`, carrying the ChangeSet row and its mutations. It records the result, so replay needs no validators or LLM, and a test replays a log with commits and reverts to the same dump hash.

**Verification.** `cargo test -p memex-core --test governance`: 17 tests, 12.9 s in release. All in memory.
1. The same idempotency key from 100 threads at once: 1 commit, 99 replays, one ChangeSet id, and the head moved by exactly one.
2. 10,000 random ChangeSets, each applied and then reverted, leave the materialized state equal, ChangeSet rows aside, since they record the history. The runs include over 1,000 page edits and over 1,000 claim replacements, asserted. Cases that can't be reverted (later edit on top, already reverted, never committed) are refused up front with nothing changed, and `inverse_preview` says why.
3. Each of the 7 always-review kinds goes to review for people and machines, with auto-commit earned or not, alone and mixed with low-risk ops. Confidence isn't an input to `risk` at all. Deletes from a server admin and edits to a verified claim go to review in full flows.
4. An unsourced machine claim is rejected and creates no assertion, including when the request says `machine: false`, because that comes from the token's principal kind. A bad quote or a missing segment is rejected. A contradictory claim commits alongside the other, linked by a `contradicts` edge and listed in `open_contradictions`. A duplicate, with case and spacing ignored, is rejected.
5. A stale base version returns 409 `stale_version` with the current row, and writes nothing. A change gone stale during review is refused at approval. A ChangeSet with a bad second op leaves its first op invisible.
6. `believed_at` reads the log, so at a past sequence it returns what was believed then, even after a later revert: a test shows a1, then a2, then a1 again.
7. Dry run: 6 combinations of proposal and author write nothing, and match a real submission's validators, risk and contradictions.

Also: reviewing needs admin scope, propose and read tokens get 403, the reviewer and reason are recorded, and other vaults stay hidden.

**Decisions.**
- `risk` lets a person's low-risk change commit directly, and a machine's only when spec 025 has earned it, through `set_machine_auto_commit`, a single switch for now. Spec 025 replaces it with the per-extractor table.
- `RemoveLink` is treated as always-review alongside deletes, as it removes something.
- Each vault's decisions run one at a time, under one lock, so idempotency and base checks hold. Across processes the writer lease covers it, and `idempotency_key` has a unique index too.
- `as_of` costs a scan of the log up to the sequence. That's correct under reverts, and fine at this size. If spec 020 finds it slow, a history table can serve it.
- Newer kinds store the full row as JSONB beside indexed columns in Postgres, which keeps SQL small while their fields still change. Spec 022 and 023 can promote fields to columns.
- `record_segment` is a stand-in until spec 015's pipeline writes segments.
