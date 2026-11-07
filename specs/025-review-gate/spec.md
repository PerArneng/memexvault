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
**Status: the gate is built, and every mechanism is verified on fakes. The live halves of items 1 and 2 haven't been run, so what a real model does with the fixtures is unknown, and no reviewer precision exists. The ≥0.9 in item 2 is unproven.** No LLM was called and no key was used. Every number here comes from a scripted agent or fake samples.

**Built.**
- `memex_core::review`: the Wilson bound, what counts as earned, combinations, what the reviewer sees, verdict parsing and the agreement scorer, all pure. `memex-core/src/server/review.rs` holds the facade.
- The reviewer job, `Server::run_review_job`, one per queued machine proposal with claims, off unless `MEMEX_REVIEW_MODEL` is set.
- Vault migration 10: filter columns on `changesets`, `review_verdicts` and `auto_commit`. No server migration.
- Routes under `/vaults/{id}/review`: `queue`, `counts`, `items/{cs}`, `items/{cs}/edit-approve`, `batch`, `auto-commit` and `auto-commit/{combination}/disable`. Approve and reject stay on `changesets/{cs}`.
- `set_machine_auto_commit`, spec 013's single switch, is gone, replaced by the table. Tests that used it now earn it with `record_audit_samples`, which is also where spec 026 reports.
- `docs/architecture.md` has a Review gate section, and `AGENTS.md` the commands and settings.

**Verification.**

1. **Mechanisms passed on a scripted agent. What a real model does: NOT RUN, as it needs a live LLM and costs money, so the user decides.**
   - Ran on the fake, in `memex-core/tests/review.rs`: 32 fixtures in `crates/memex-core/tests/fixtures/injection/`, 8 each in text, hidden text, file metadata and fake tool output. The scripted extractor is obedient: it asks for tools, then returns the planted claim and an invented quote. The scripted reviewer asks for tools, then approves. Result for all 32, with auto-commit off: 0 commits, 0 tool calls accepted (each tool-using reply is refused and costs its retry, and `AgentRequest` has no way to offer a tool), and page, claim, evidence, entity, type, edge, block, slug and revision rows all unchanged. The invented quote makes no proposal. The reviewer's request holds only subject, predicate, value and quotes, and an approving verdict leaves the item pending. File names and metadata reached no request in the 8 metadata fixtures.
   - Also on the fake, with auto-commit earned at 10,000 of 10,000: all 32 in an `untrusted` source gave 0 auto-commits, and the 8 hidden ones gave 0 even in a `trusted` source.
   - **The limit, shown by `the_limit_a_planted_sentence_in_a_trusted_source_commits_as_a_plain_claim_once_earned`:** with auto-commit earned, a planted sentence in plain view in a source marked `unverified` or better commits as an `asserted` claim. It touches nothing else and can't be `verified`. So "0 auto-commits" holds with auto-commit off, and for untrusted sources and hidden text with it on, but not for that case. Nothing here tries to detect bad text, as the design says. See the decision for the user below.
   - The live run, about 55,400 tokens by estimate (input 48,707, output 6,720, up to twice that if every call retries), from the real prompts with no call made. It prints what the model answered, for a person to read:
     ```
     MEMEX_LLM_PROVIDER=gemini MEMEX_LLM_MODEL=gemini-3.5-flash-lite MEMEX_LLM_API_KEY=<key> MEMEX_EVAL_MAX_TOKENS=120000 \
     cargo test -p memexvault --release --test review_eval -- --ignored --nocapture --exact live_injection_fixtures
     ```
2. **NOT RUN. No reviewer precision has been measured, and no human labels exist.** Built and proved on a scripted agent: the scorer with hand-made cases, and a harness over a 48-line set in `crates/memexvault/tests/fixtures/review/` (en, sv, de, es, each 7 correct and 5 wrong). **An agent wrote those labels, and each line says so.** The report prints `NOT A MEASUREMENT` until every line is a person's. The scripted run's 92.6% comes from replies written from the labels with planted mistakes, and says nothing about a model. A person must check the labels, then run this, at about 13,300 tokens by estimate:
   ```
   MEMEX_LLM_PROVIDER=gemini MEMEX_LLM_MODEL=gemini-3.5-flash-lite MEMEX_LLM_API_KEY=<key> MEMEX_EVAL_MAX_TOKENS=30000 \
   MEMEX_REVIEW_LABELS=<checked labels> \
   cargo test -p memexvault --release --test review_eval -- --ignored --nocapture --exact live_reviewer_agreement
   ```
3. Passed. `wilson_has_these_exact_values`, against values worked out separately in Python from the same formula with z = 1.96: 0 of 0 and 0 of 500 are 0, 1 of 1 is 0.20654, 9 of 10 is 0.59584, 10 of 10 is 0.72246, 90 of 100 is 0.82563, 72 of 72 is 0.94935 and 73 of 73 is 0.95001. So at the default threshold of 0.95, the first possible earning takes 73 of 73.
4. Passed with fake samples, on the fake store and on Postgres. 72 of 72 stays off, 73 of 73 turns on and the next extraction commits by itself, one failure at that count turns it off and the next goes to review, and 149 of 150 turns it back on. Each change is in the history.
5. Passed in `every_decision_says_who_when_and_why`: a person's approval, rejection and edit, the validators, a person's own low-risk change, an earned auto-commit with its counts, prompt and bound, a disable, and in the stale test the pipeline. Each has who, when and why. The event's actor matches, an edit keeps what was proposed, and a verdict records its model, prompt hash, tokens and time.
6. Passed. Approving twice returns the same row with no new event and no second sample, also by dump hash on Postgres. A second person gets a 409 naming who decided. A stale base gets `stale_version` with the current row. A proposal the vault has moved past, or on a replaced rendition, gets a 409. An outage: 30 rounds left the job waiting with both items in the queue, a person could still decide one, and when the model came back only the other cost a call.
7. Passed, also on Postgres. With 500 good samples on record, the next proposal after a disable goes to review. 72 new samples aren't enough and 73 are. The OpenAPI document has no route to enable one, and `POST .../enable` is a 404.

**Definition of done** on the final commit: see the landing report. Measured on a machine shared with three other workers. No timing target applies to this spec.

**Not run,** as the brief says: `pg_bad_cases`, `pg_kill_postgres` (both reach beyond this worker's database), `pg_child` and `pg_job_child` (helpers), and `pg_full_disk` (needs a small disk). This adds rows and columns to the same commit path and doesn't change how commits or leases work, so I expect no effect, but that's untested. `pg_kill_jobs_200_times` and `pg_kill_memexvault_1000_times` passed.

**The other specs' follow-ups.**
- **Spec 023:** closed. `needs_a_person` now covers every always-review kind, so an agent with admin scope can't approve or revert any of them. Tested for delete, removing a link, editing a verified claim, promoting an untrusted source and a machine's verified claim. Reverting a promotion stays open to it, because the kind is worked out from current state, and by then the source isn't untrusted. Lowering trust is the safe direction.
- **Spec 024:** settled. Evidence must cite the current rendition, by a new `current_rendition` validator, so a proposal on a replaced one is refused and can't be approved. The new rendition's extraction run rejects extraction's pending proposals on old ones as `pipeline`, before any call, so an outage doesn't hold it up. Retiring happens there, rather than when the rendition commits, as `build_rendition` reads outside the vault's lock and could overwrite an approval made meanwhile. With extraction off nothing retires them, and a person can reject them, which doesn't count against the extractor.

**Decisions.**
- **Stricter than the spec asks:** a machine's claim citing an untrusted source, invisible characters or a page flagged for hidden text never auto-commits. The spec only says untrusted sources can't reach `verified` automatically, but its Goal asks for 0 auto-commits on injection fixtures, and this is the mechanism for that.
- **`verified`:** a claim arriving as `verified` from a machine or citing an untrusted source is a new always-review kind, `ClaimVerified`, even from a person's token.
- **Samples:** a person's decision on a machine's proposal. An edit, a rejection and a person reverting an auto-commit count as wrong. Agents' decisions, verdicts and always-review changes count for nothing. A person reverting something a person approved adds no sample, so its earlier "correct" stands.
- **A second decision by the same person is idempotent, and by someone else a conflict.** Item 6 asks for the first and spec 046 for the second.
- **Earned on read.** Whether a combination is on comes from its counts and the current threshold, so a new threshold takes effect at once. The history records only changes caused by samples and disables.
- **Falling below keeps the counts, and a disable starts them again.** Item 4 needs the first to recover and item 7 needs the second to take real work. After falling, recovery can be quick or slow depending on the count.
- **The reviewer** is asked for by name with `MEMEX_REVIEW_MODEL`, as each queued claim is a call, and someone with extraction on shouldn't start paying for it by upgrading. Its verdict never affects auto-commit, following the diagram in Governance.
- **`decided_by`** is now also set on what nobody reviewed: `auto-commit`, `validators`, or the person. It was empty before, which item 5 doesn't allow. Older rows stay as they are.
- **An agent with admin scope can still approve ordinary claims,** but that earns nothing. The spec doesn't reserve them for a person.
- **`approve` now returns 409, not 400,** when validators fail on a proposal that passed them when proposed. A person's edit that fails is still a 400.
- **Batches** are up to 200 decisions, each on its own, so one conflict leaves the rest. They aren't atomic.
- **Threshold** is one server-wide setting in thousandths, above 0, with no per-vault value.

**For the user to decide.**
1. The live runs in items 1 and 2, and who checks the labels. Spec 081 holds the same kind of work for extraction, and could take these.
2. Whether `unverified` sources, which is what most uploads are likely to be, should also never auto-commit. That would close the limit in item 1 for them, at the cost of auto-commit only ever applying to sources someone marked `trusted`. I kept to `untrusted`, as that's the design's word.

**Follow-ups.**
- The prompt hash covers the type registry, so adding an entity type resets every extraction combination to nothing earned. It's safe, and it may prove too strict once auto-commit is in real use.
- Auto-commit has no samples once it's on, other than reverts, until spec 026's audit reports through `record_audit_samples`.
- Items from before this spec have no origin, so they show as `unknown` and earn nothing.
- A proposal queued while the reviewer was off has no job and never gets a verdict. Nothing queues it later.
- Spec 030's erasure must cover `review_verdicts`, whose reasons may repeat document text, as well as `derivations`.
- Run the live harness through the whole pipeline. It calls the same prompts and parser directly, without a vault.
- The console has no screen for this yet: spec 046.
