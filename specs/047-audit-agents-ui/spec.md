---
id: 047
name: audit-agents-ui
description: "Build the audit agent's dashboards with runs, findings, schedule, budgets and cost"
---

# 047 audit-agents-ui

## Goal
A user can see what the random checks found, how much to trust them, and what agents cost, and can start a run.

## Background
**Depends on:** 026, 039, 040. Design: `docs/architecture.md`, section Audit agent.

## Approach
- Run list and detail with seed, sample and budget used. Findings with each check's result, linked to the claim, source and the resulting review item.
- An on-demand run with seed and budget, and a schedule editor.
- Recall report from planted errors, with its sample size stated, and a warning when it's too small to trust.
- Agent activity and token use by agent, model and day. Cost only where a price is configured.
- Charts follow one chart style from the design system, with tables for exact numbers.

## Verification
1. A run started from the UI appears live, finishes, and its findings link to their review items, in Playwright with a scripted LLM.
2. Rerunning with the same seed shows the same sample.
3. Totals on screen match the API for a fixture month, including when no price is set, where no cost appears.
4. A recall figure with too few planted errors shows the warning.
5. A budget that runs out shows as stopped by budget, not as failed.
6. Bad cases: an invalid schedule is refused with a clear message, and a read-scope user can view but not start runs.

## Out of scope
Auto-fixing findings, reputation and external alerting.

## Result
**Built.** An Audit screen at `/vaults/{id}/audit`, lazy loaded, replacing the shell's placeholder, with four tabs. It needs read scope to view.
- **Runs:** every run, newest first, with state, trigger, seed, sample against population, findings and the budget used. A form starts a run with a seed, sample size and token budget, each left to the schedule when empty, for admin scope only. An open run is read again every second and the list every 10 s, so a run moves on screen with no reload.
- **A run:** its state in words, seed, sample asked and taken, budget used, model, passes, suspects and failures, LLM calls, combinations it turned off, siblings sent back, vault-level findings, and the sample in rank order. Below it, every sampled claim or only the findings, each with its checks' results, a link to the claim's lineage, its sources (read when a finding is opened, each linking to the viewer at the quote) and its review item, or why it has none. Check details hold a document's or a model's words and show as plain text.
- **Schedule and budget:** the schedule in words, an editor for admin scope that names every problem by the server's own rules before sending and shows the server's refusal if it still comes, and today's spending against the daily budget.
- **Recall:** overall recall with its sample size stated, the deterministic and LLM-only groups apart, per kind, false positives, and whether the target is judged. A warning shows when too few errors were planted, when the LLM figures are scripted, and when no LLM has answered.
- **Agent usage:** tokens by agent and model for a month, a chart by UTC day and agent, and a table by day, agent and model with exact numbers. A cost shows only where the server has a price, and "No price set" otherwise.
- **The chart style:** `StackedColumnChart` in `@memex/ui`, on new tokens `chart-1` to `chart-5` and `chart-grid`. `pnpm run contrast` now holds each series at 3:1 on every surface (73 pairs), and its token pattern now reads names with digits. It's in the gallery. Recorded in `docs/frontend-architecture.md`.

Pure logic is in `packages/core/src/audit.ts`, `audit-usage.ts`, `audit-routes.ts` and `audit-cases.ts`, with a fake in `testing-audit.ts`, wire translation in `packages/api/src/translate-audit.ts`, and the views in `apps/console/src/audit/`.

**Decisions, by the user through the coordinator.**
- **Audit runs count in the usage report (option 1a).** `/system/usage` summed Derivations only, and audit runs keep their tokens on the run, so the audit agent's own spending never appeared. `usage()` now also folds each run that called a model in as agent `audit`, by its model and the UTC day it was created, priced like any row. Additive, with no migration: `Spent` in `memex-core/src/server/admin.rs` stands for a Derivation or a run, and `audit_runs_count_as_the_audit_agent_by_their_day_and_model_and_runs_with_no_model_are_left_out` tests it. `docs/architecture.md` says so. **Follow-up:** the reviewer agent's calls aren't in Derivations either, and still aren't counted.
- **Usage stays a server admin's (option 2a).** The tab reads `/system/usage` filtered to the vault for a server admin, and tells anyone else it needs one. No scope changed.

**Verification.** `pnpm --filter @memex/console e2e:audit`, 7 passed in 35 s on Chromium, against a debug build of `memexvault serve` on this worker's own Postgres (a pgvector 0.8.7 container on port 55447) and bind `127.0.0.1:18477`, at load average about 3. The LLM is a stand-in server on this machine (`e2e-audit/stand-in-llm.ts`), set as `MEMEX_LLM_PROVIDER=openai` with `MEMEX_LLM_MODEL` and `MEMEX_AUDIT_MODEL`, so nothing called a provider. The fixture is a test vault with 6 sources and 24 machine claims a person approved, with 14 errors planted through the API, one of each kind.
1. **Passed.** A run started from the form showed Queued and then Finished 1.9 s after Start with no reload (checked by listening for page loads), with 3 failed and 21 suspect from 24 sampled and 23 LLM calls, the same figures as the API. Its 24 findings each link to their review item, which the API shows as a pending `retract_assertion`, and following one lands on it in Review. The stand-in's `not_entailed` for the claim it was told to refuse is among them. The source's hostile HTML ran nothing.
2. **Passed.** Seed 42 again gave the same 24 claims in the same order on screen and through the API, and seed 43's first 10 differ.
3. **Passed.** For the current month (2026-11-01 to 2026-11-30), the screen's calls or runs, input, output, thinking and total tokens equal the sums of the API's rows for the vault: 2 rows, `audit` and `extraction`, 9 calls or runs and 3,200 tokens. With `MEMEX_LLM_PRICES` pricing only the extraction model, extraction shows $0.0016, the total shows it, and the audit row says "No price set". After restarting the server with no prices, the API gives no cost on any row and no `$` appears.
4. **Passed.** With 14 planted errors, the API's `too_small` is true and the Recall tab warns "Too few planted errors to trust this figure." and states 14 planted.
5. **Passed.** A run with a budget of 1 token finished with `stop=run_budget` and 0 LLM calls, and shows "Stopped by budget", saying running out isn't a failure, in the run and in the list, with no "Failed" anywhere.
6. **Passed.** An interval of 0 and a sample of 5000 are refused in the form with both reasons, and the schedule is unchanged. The API refuses `interval_ms` 1000 with 400 "the interval must be at least a minute". A good schedule saves. A read token sees runs, a run's findings, the schedule and recall, gets no Start or Edit, is told usage needs a server admin, and the API answers 403 to starting a run and to writing the schedule.

axe found nothing on the runs list, a run with a finding open, recall, usage, the schedule and the read-only view.

Unit tests: 39 in the five new test files (access, run states, budget words, the trigger and schedule forms and their bad cases, recall warnings, months and leap years, usage sums with and without prices, routes, paging and its bound, translation). `pnpm test` runs 801, all passing.

Also run: `cargo build --workspace`, `cargo test --workspace` (1038 passed, 0 failed, 126 ignored that need Postgres), `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check` and `cargo deny check licenses`, all clean; and from `web/`, `pnpm build`, `test`, `typecheck`, `lint`, `run licenses`, `run contrast` (73 pairs, 0 failed), `budget` (150,147 bytes gzip of 200,000; the audit screen is its own 10 kB chunk) and `api:check` against the running server, all passing. The OpenAPI document changed only in the usage route's descriptions.

**Limits and follow-ups.**
- **The recall report counts a stand-in as a model.** It treats any model name but `scripted` as a real model, so this fixture's report says `llm_evidence` `model`. Its target stays unjudged here only because too few were planted. Nothing in this spec relies on it, and the report's rule is spec 026's.
- The run list reads at most 4,000 runs, ten years of a daily schedule, as the API lists oldest first.
- Live updates poll rather than follow the activity stream. An audit's frames don't say what a run found, so the screen would read the run anyway.
