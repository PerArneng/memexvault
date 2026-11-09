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
**Built.** A Review screen at `/vaults/{id}/review`, lazy loaded, replacing spec 044's placeholder, which wiki deletions now link into. It has four tabs.
- **Queue:** filters (extractor, prompt version, item type, proposer, verdict and source) with counts by kind, a list beside the item, the claim beside its source region in spec 042's viewer with the quote highlighted from rects, the reviewer agent's verdict, validator results and the automatic path in plain words. Approve, reject with a reason, edit then approve (the ops as JSON), and batches of selected items. Every action has a key: `j`/`k` or arrows, `a`, `n` (approve with a note), `r`, `e`, `x` (select), `X` (select all shown), `A`, `R` (batch approve and reject), `s` (the item's next quote), `o` (open the source in Documents), `?` and `Esc`. Counts follow spec 038's activity stream.
- **Changes:** the ChangeSet list (oldest first, the only sort the API has) and detail with ops, author, rationale, decision and reason, inverse preview and a revert, confirmed in the page.
- **Contradictions:** open ones with both claims and their sources, each linking to the viewer.
- **Auto-commit:** per combination its samples, precision, Wilson bound against the threshold, state in plain words, revocation history, and a manual disable confirmed in the page. The rules the user decided are stated above the table: only sources a person marked trusted auto-commit, chat proposals never do and add no samples, and one failed audit turns a combination off.

Pure logic is in `packages/core/src/review.ts`, `review-routes.ts` and `review-cases.ts`, wire translation in `packages/api/src/translate-review.ts`, and the stream in `packages/edge-sse/src/queue-feed.ts`, its own file as agreed. Views are in `apps/console/src/review/`. `Principal` gains `machine`.

**Backend, additive and with no migration.**
- **An agent decides nothing through a session** (the coordinator's decision, option a). An agent principal on a session cookie gets 403 `agent_session`, whatever its scope, from approve, reject, edit-then-approve, batch and revert: `POST /changesets/{id}/approve`, `/reject`, `/revert`, `/review/items/{id}/edit-approve` and `/review/batch`. The code and its meaning are in the OpenAPI document's 403 for each. `disable_auto_commit` isn't covered: it only ever makes things safer, and the console hides it from agents. `Caller` gains `via_session`. **The remaining gap: an admin-scope agent with a bearer token can still approve ordinary claims**, as spec 025 recorded, and both the Rust test and the browser check show it.
- **A pending quote has rects.** `ReviewEvidenceDto.rects` is found read-only with `locate_within`, the locator that anchors evidence at commit, best effort, so a quote it can't place has none and the item still opens. It's computed on item detail only, never per list row, so a long queue costs nothing extra. `a_pending_quote_gets_the_same_rects_it_is_anchored_to_at_commit` shows they equal what `anchor_new_evidence` stores.
- **`dump_vault`**, a read-only example in `crates/memexvault/examples`, prints `canonical_dump` of a vault's knowledge tables, `Source`, `Page`, `Edge`, `Segment`, `Assertion` and `Evidence`, leaving out head, changesets and jobs. These are the tables `state_without_changesets` in `memex-core/tests/governance.rs` compares, so item 3 mirrors it. It only reads.
- The API types were regenerated from a running server.

**Verification.** The browser checks ran on Chromium only, against a debug build of `memexvault serve` on this worker's own database, with `MEMEX_AUTO_COMMIT_THRESHOLD=0.2` so one approval earns a combination: `pnpm --filter @memex/console e2e:review`, 16 passed in 1.5 min, at load 8.9 at the start and 54.9 at the end, from other workers, so the timings are weak.
1. **Passed.** 20 fixture items, every kind of decision (8 approved, 3 approved with a note, 4 rejected with a reason after an empty one was refused, `Esc` backing out of a form, 1 edited then approved, 2 batch approved and 2 batch rejected, moving with `j` and `k`), by keyboard alone, in 11.2 s including waits for the screen. A second check lists every key and shows none acts while typing.
2. **Passed.** Approve, reject and edit then approve each record the signed-in person as `decided_by` and the typed reason as `decision_reason`, read back through the API. An edit keeps what was proposed. A revert keeps its reason in its rationale, as spec 025 has it, and the console shows it there. A further check: an edit the server refuses keeps what was typed in the form and decides nothing.
3. **Passed.** `dump_vault` before, after committing and after a revert from the console: 25652, 26775 and 25652 bytes in 41 lines, and before and after the revert are byte for byte the same.
4. **Passed.** A page deletion and a claim citing a source below trusted show "Only a person can approve this" and no automatic path, the latter even with its combination earned. A trusted source's claim auto-commits once earned and the screen says why. An admin-scope agent signed in through a session sees no decision controls, its keys are refused with a reason, and the server answers 403 `agent_session` on all five routes, with nothing decided; the same agent's bearer token approves and reverts an ordinary claim, as spec 025 recorded. `an_admin_agent_decides_nothing_through_a_session_and_keeps_what_its_bearer_token_may_do` in `memex-http/tests/session.rs` checks the same against the server, and that a person's session still decides. **A chat proposal's words and an audit's revocation in the table were shown with the server's answer rewritten in the browser,** as the server can't produce a chat proposal without an LLM or a revocation without a damaged source, and the unit tests cover both. A real chat proposal through the screen is not run.
5. **Passed.** Two reviewers on one item: the second, with a stale screen, reads "This couldn't be done, and nothing changed. this ChangeSet is already Committed, decided by" and the first's id, and the item has one decision.
6. **Passed.** Hostile text in a claim's ops, its rationale, a reviewer verdict and a validator result (the last two rewritten into the server's answer in the browser, as only a model makes them), and in a decision's reason, shows as plain text on the queue, the item, the edit form and the Changes list and detail. No script ran, no element was made from it, nothing went to another origin, and the console logged no errors.
7. **Passed.** 88 unit tests in the five files of this spec: queue keys, steps, selection, reconciling with the stream, batches and their limit, parsing edited ops, decision access, outcome words, routes and the automatic path, each with bad cases. `pnpm test` runs 461 in all, all passing.

Also checked: the claim beside its highlighted source on a PDF and in plain text; a new proposal reached the count with no reload in 298 ms (load as above, weak); a link from the wiki lands on its item; axe finds nothing on any tab, and no CSP error shows up. No modal was added, so no new CSP errors.

**Definition of done**, on the final commit:
- `cargo build --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check` and `cargo deny check licenses` passed. `cargo test --workspace`: 978 passed, 0 failed, 105 ignored, at load 20 to 66.
- The Postgres suite on a freshly dropped and created database of this worker's own, `cargo test -p memexvault --release --no-fail-fast -- --ignored --test-threads=1 pg_` with `pg_child`, `pg_job_child`, `pg_full_disk`, `pg_kill_postgres`, `pg_bad_cases` and `tls_` skipped: 47 passed, 0 failed, in 18 min at load 7 to 36, with no `LeaseLost`. **Those six were not run**, as the brief says, since they kill or fill the shared server or need a second one. This change can't affect what they cover: it adds a check on who decides through a session, a read-only lookup on item detail and a read-only example, and touches no jobs, storage, connection handling or TLS.
- `pnpm typecheck`, `lint`, `test` (461), `run licenses`, `run contrast` (53 pairs, 0 failed) and `pnpm e2e` (6) passed. `pnpm budget`: the sign-in route's initial JavaScript is 125529 bytes gzip, under 200000. The review chunk is 43.83 kB, 11.70 kB gzip.
- `e2e:shell` 17 passed and `e2e:wiki` 17 passed. `e2e:documents`, with `MEMEX_E2E_PG_CONTAINER` set so its SQL acts on this worker's database only: 9 passed, 1 skipped, and **item 5 failed**, a median of 1786 ms to the first warm page of 400 against 1000 ms, on the debug build spec 042 says misses it. Without the variable item 6 fails too, at the step that needs it.

**Decisions.**
- Forms stay open until the server answers, so a refusal loses nothing typed. A plain approval moves on at once and comes back if refused.
- The queue list isn't virtualized: it loads pages of 200 with a button for more. A filtered queue rarely holds more, and it can be added if one does.
- The Changes list is oldest first, the only sort `GET /changesets` accepts.
- `disable_auto_commit` isn't guarded by the session rule, as above.

**Follow-ups.** The console's queue feed is its own small file, and spec 041's feed is expected to land first, so this merges onto it at landing. A newest-first sort on `GET /changesets` would make the Changes list more useful.
