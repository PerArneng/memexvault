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
Built and landed with item 2 not met yet, by the user's decision: its deterministic group measures 80 of 80, and its target depends on a live LLM run that's owed, now in spec 081. Items 1, 3, 4, 5, 6, 7 and 8 pass. Every LLM answer here came from a scripted agent, and no provider was called.

**What was built.**
- `memex-core/src/audit.rs` is the pure part: sampling, the schedule, budgets, the checks and the recall report. `server/audit.rs` is the job, the scheduler step and planting.
- New rows and tables: `AuditRun`, `AuditCheck`, `AuditState` and `AuditPlanted`, in vault migration version 12. A finding is an `AuditCheck` that didn't pass.
- New op: `RetractAssertion`.
- New trait methods with defaults: `LexicalIndex::reconcile`, `VectorIndex::reconcile`, `Snapshot::id_digest`, `Snapshot::audit_checks` and `Snapshot::claim_counted_correct`. Postgres overrides the last three in SQL.
- Routes under `/vaults/{id}/audit`: `runs` (list and trigger), `runs/{run}`, `runs/{run}/checks`, `schedule` (read and write), `budget`, `recall`, `test-vault` and `planted`.
- `AuditHandler` and a scheduler thread in `workers.rs`. Settings `MEMEX_AUDIT_MODEL`, `MEMEX_AUDIT_OUTPUT_TOKENS` and `MEMEX_AUDIT_TIMEOUT_S`.
- The console's API types are regenerated, and `docs/architecture.md` and `AGENTS.md` are updated.

**Verification.** Timings are from a machine at load average 680 to 940, from other work.

1. **Passed.** The same seed and state give an identical sample, and another seed differs: `the_same_seed_and_state_give_an_identical_sample_and_another_seed_differs`, unit tests in `audit.rs` that shuffle the candidates, `a_later_clock_does_not_change_the_sample`, and on Postgres `pg_audit_same_seed_and_state_give_the_same_sample_on_a_schedule_or_on_demand`.
2. **Not met yet.** 112 errors planted, 8 of each of 14 kinds, plus 60 clean claims, on Postgres and tantivy (`pg_audit_planted_errors_*`) and on fakes, with the same numbers.

   | Kind | Group | No LLM | Scripted LLM |
   |---|---|---|---|
   | altered_quote, wrong_segment, moved_anchor, broken_anchor, missing_segment, missing_source, missing_blob, stale_ttl, expired_validity, index_missing | deterministic | 8/8 each | 8/8 each |
   | wrong_value, negated_value, wrong_subject, unsupported_claim | LLM only | 0/8 each | 8/8 each, mechanism only |

   - **The deterministic group is a real measurement:** 80 of 80 caught, 100%, with 0 false positives on 60 clean claims.
   - **The LLM-only group has no real number.** With no LLM it catches 0 of 32, so recall over everything planted is 80 of 112, or 71.4%, which is a floor. With scripted answers it catches 32 of 32, which proves the path from an answer to a finding and says nothing about a model, whose answers were written by the same test. The LLM false-positive rate is unmeasured for the same reason.
   - **What's owed:** the 80% overall target can't be judged without a live run. The report's `target_met` stays null until every kind is planted and a real model has answered. The run below, which needs the user's decision, is NOT RUN:
     `MEMEX_LLM_PROVIDER=... MEMEX_LLM_MODEL=... MEMEX_LLM_API_KEY=... MEMEX_EVAL_MAX_TOKENS=600000 MEMEX_TEST_DATABASE_URL=... cargo test -p memexvault --release --test audit -- --ignored --nocapture --exact live_planted_recall`
   - **Token estimate:** 208 calls and about 136,000 input tokens, as counted on the scripted run at 4 characters a token. Output is capped at 2,048 per call, so at most about 426,000 more, and typically far less. Thinking tokens would add to that on a model that uses them. It sends only the fixture's made-up sentences. A run with a model must also be read for false positives. Its answers may not put re-extraction and entailment in the proportions the script did.
3. **Passed, end to end with fakes and on Postgres.** A failure at 10,000 correct and 1 wrong turned the combination off, reset its counts, kept the totals and recorded the audit as the cause, and 5 of 5 siblings were requeued with no claim changed: `a_failure_revokes_auto_commit_and_requeues_siblings_end_to_end`, `pg_audit_failure_revokes_requeues_and_a_person_decides`. Above the cap, 449 siblings went 200, 200 and 49 across three runs, with 249, 49 and 0 reported as left, and a fourth run queued nothing: `siblings_past_the_cap_are_counted_and_later_runs_carry_on_until_covered`.
4. **Passed.** Run and day budgets stop the run with `stop` recorded and the job `done`, every claim keeps its deterministic results, a call or retry that doesn't fit isn't made, the day's spending carries across runs and resets the next day: `the_run_budget_stops_the_run_cleanly_and_records_that`, `the_day_budget_holds_across_runs_and_starts_again_the_next_day`, `a_retry_that_would_not_fit_is_not_made_and_the_tokens_are_on_record`, `a_budget_of_nothing_makes_no_calls`.
5. **Passed.** The audit agent's caller, a machine with `propose` scope only, gets `Forbidden` on approve, reject, edit, revert, batch, disable, trigger, schedule, test vault, planting and direct writes, in `the_audit_agent_can_propose_and_nothing_else`. Over HTTP, a real propose-scope agent token gets 403 on each write route, in `an_agent_with_propose_scope_is_refused_every_direct_write_and_a_read_token_every_write`. An item stays pending however much is earned, in `an_audit_item_stays_a_review_item_however_much_the_audit_has_earned`.
6. **Passed by the rule set beforehand, with one bad pair.** 200k segments and 10,000 claims, hybrid search with caches off and 4 clients, `audit_search_latency`. Median ratio 1.36 against a limit of 1.5. The rule uses the median, so pair 3 doesn't fail it, but pair 3 was far over the limit and its cause wasn't investigated. It could be the audit's scan or the machine, as its throughput fell to 143 searches.

   | Pair | Idle p95 ms | Audits p95 ms | Ratio | Load average before |
   |---|---|---|---|---|
   | 0 | 231.1 | 261.3 | 1.13 | 811 |
   | 1 | 266.0 | 276.1 | 1.04 | 784 |
   | 2 | 435.3 | 500.9 | 1.15 | 746 |
   | 3 | 253.5 | 3571.2 | 14.09 | 751 |
   | 4 | 234.9 | 318.5 | 1.36 | 721 |
   | 5 | 214.2 | 302.6 | 1.41 | 676 |

   These p95s are not the target's numbers, as the machine was far over capacity. One audit of 200 claims took 45 s, so each 30-second round held about one, and an audit was running for all of it. The harness's sources have fake hashes, so each sampled claim counted as a dead link and filed a review item, which is more writing than a real audit does (200 suspects in the first). No LLM was called, as the rule says.

   Nothing else was measured at 50k, so there's no growth figure from this spec. Reading every claim on one snapshot is linear in claims, so the scan would be about 5× longer at 1M segments' 50,000 claims. Spec 033 should measure that.
7. **Passed, on `ManualClock` with no sleeping.** `nothing_runs_until_a_schedule_is_turned_on_and_due`, `an_empty_vault_finishes_cleanly_with_nothing_sampled`, `an_llm_outage_leaves_the_deterministic_checks_done_and_the_run_finished` (the broken quote is still found and still revokes, after one try at the endpoint), `a_provider_refusal_also_stops_the_llm_checks_and_says_so`, `a_missed_schedule_runs_once_and_says_how_many_slots_it_passed_over`, `a_slot_that_finds_a_run_still_open_is_passed_over_and_counted`.
8. **Passed.** `a_run_through_the_route_samples_the_same_as_a_scheduled_run_with_that_seed`: the same 10 claims in the same order through `POST .../audit/runs`, and another seed differs.

**Postgres suite.** 33 passed and 0 failed on a freshly recreated database, with the skips the shared server needs. `pg_child`, `pg_job_child`, `pg_full_disk`, `pg_kill_postgres`, `pg_bad_cases` and the `tls_` tests were not run. The last two need the coordinator at landing. This spec adds tables and rows, which `pg_bad_cases` and `pg_kill_postgres` cover only as ordinary commits, and it changes nothing about connections or TLS.

**A retired claim, in every reader.** Checked on fakes and, for search and revert, on Postgres: a page's reference reads as `Missing` (`server/pages.rs` follows `superseded_by`, so this needed a fix), entity fields fall back and the card drops the name, the keyword and vector indexes drop it, `as_of` and `believed_at` find it only at a time it stood, a cached search can't serve it, and a revert restores the row exactly. Reconciliation also agreed after a retirement on both indexes.

**Reconciliation at scale.** At 200k, the store's SQL digest matched tantivy and pgvector for 10,000 claims and 200,000 segments. That also checks the SQL and Rust hashes against each other on real data.

**Decisions.**
- **`RetractAssertion`, by the coordinator's decision, which the user can overrule before landing.** Findings and siblings are proposals of it, it's always reviewed, and only a person can approve it. Approving retires the claim and rejecting keeps it. Samples: keeping is +1 correct, retiring a sibling or a re-extraction disagreement is +1 incorrect, and retiring a finding the audit counted adds nothing. A claim is `Superseded` with no `superseded_by`, a new state.
- **What revokes.** Only a quote failing on the rendition the claim cites (deterministic) or `not_entailed` (an LLM check), and only for a machine's combination. Link rot, a replaced rendition, a broken or moved anchor, staleness, a re-extraction disagreement alone and index drift are findings. One failure revokes however good the record, through `record_audit_samples`' code and `review::disabled`, as a bound alone would have left 10,000 and 1 on.
- **A correct sample needs an entailment,** and a claim gives one only once. A quote that's where it says proves nothing, and counting it, or counting the same claim twice, would earn auto-commit from nothing. So with no LLM, audits can revoke and can't earn.
- **Age is measured from the vault's newest commit,** not the clock, or item 8 couldn't hold.
- **A retirement may leave an entity without a required field,** since a schema mustn't keep a wrong claim in place.
- **Small choices.** An outage ends the LLM checks rather than waiting, as the next run samples again. A claim gets one finding item ever, and one a person kept isn't asked about again. An index-only finding has no review item, since the claim is fine. A schedule is off by default. A test vault is marked while empty and stays one. `apply_as` refuses the audit agent by name, and other agents are unchanged, since it's on no route. A sibling's cap counts new items only.

**Limits and follow-ups.**
- A false sentence quoted faithfully from a trusted source passes every check, and finding it would take comparing sources.
- Blind re-extraction's comparison, by value and subject, is untested against a real model and may be too strict or too loose. The live run is the first evidence.
- Sampling holds every standing claim in memory, about 200 bytes each. A candidate query in SQL would cut both that and the scan if spec 033 shows a need.
- A broken page link shows on the run with no review item, since `RemoveEdge` wouldn't fix the page's text.
- The rationale and check details hold document text, and spec 047 must show them as plain text.

**Item 6's rerun, by the coordinator, on this branch merged with main, before landing.** The same rule and command, on a fresh database, with the load average at 13 to 47. Passed: median ratio 1.25 against 1.5. The median is the upper of the middle two, as the harness takes it.

| Pair | Idle p95 ms | Audits p95 ms | Ratio |
|---|---|---|---|
| 0 | 48.8 | 72.0 | 1.47 |
| 1 | 57.1 | 71.5 | 1.25 |
| 2 | 50.6 | 66.0 | 1.31 |
| 3 | 59.7 | 68.4 | 1.15 |
| 4 | 62.9 | 61.7 | 0.98 |
| 5 | 57.4 | 60.3 | 1.05 |

The earlier 14.09 didn't come back, which fits the overloaded machine as its cause, but its cause still wasn't investigated. One audit took 5.5 s, against 45 s before. Every sampled claim was still suspect because of the harness's fake source hashes, so these audits write more than real ones would.

**Landing checks,** by the coordinator on the same merge:
- `cargo build --workspace`, `cargo test --workspace` (768 passed, 0 failed, 79 ignored), clippy, `cargo fmt --check` and `cargo deny check licenses` all pass.
- The Postgres command passes 33 of 33 on a fresh database.
- `pg_bad_cases` passes on a throwaway container, and so do the 8 TLS tests on their own server.
- `pg_kill_postgres_100_times`, on a throwaway container: 100 cycles, highest acknowledged 6714, lost 0, partial 0, gaps 0, reopen failures 0, recovery p95 0.4 s. It works on the store alone, and doesn't run an audit.
- A server built from this code gives the committed API types with no change. `pnpm api:check`, `pnpm typecheck` and `pnpm test` pass.
- Not run: `pg_child`, `pg_job_child` and `pg_full_disk`.

**The user's decisions at landing:** land with item 2 unmet, and keep a single failure revoking auto-commit, even at 10,000 correct and 1 wrong, at least until the live run shows how often a real model is wrong.
