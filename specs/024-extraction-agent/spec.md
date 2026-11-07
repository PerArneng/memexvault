---
id: 024
name: extraction-agent
description: Extract claims and entities from renditions with an LLM, with every result anchored to its source
---

# 024 extraction-agent

## Goal
An ingestion stage that turns segments into proposed claims and entities of known and dynamic types. Every committed claim
resolves to a source region, with precision ≥0.9 on a labelled set.

## Background
**Depends on:** 007, 016, 023. Design: `docs/architecture.md`, sections Governance, Prompt injection, Anchors, Languages.

## Approach
- `AgentRunner` trait in core, a scripted fake for tests, and `memex-agent-adk` as the only ADK dependency.
- A job per Rendition, batching segments. The prompt carries the type registry's schemas, and output is schema-only with no
  tools.
- The model returns segment id, quote, claim fields and type. Claims stay in the source language, and quotes are verbatim. The locator derives rects. A quote that can't be found fails
  validation, so a made-up quote can't enter the system.
- Results become Proposals with a Derivation holding model, prompt hash and inputs. Everything goes to review by default.
- Token budget per document, retries on bad output, and waiting and retrying while the LLM is down, while everything else
  stays available.
- Identity resolution against existing entities produces candidates, not merges.
- Derivations record token counts. Routes for per-document extraction status, including partial and waiting for the LLM, and a re-run.

## Verification
1. Labelled set of at least 50 documents using public material, with at least 10 each in English, Swedish, German and Spanish, run
   live once: precision ≥0.9, recall recorded, per type and per language. The
   token cost is recorded. Live runs use only documents approved for sending to the provider.
2. 100% of committed claims resolve to anchors, checked by query.
3. Scripted-fake tests: invented quotes are rejected, wrong schema output retries then fails, and an LLM outage leaves jobs
   waiting with search unaffected.
4. Rerunning the same Rendition and prompt creates no duplicates.
5. A new type added to the registry is extracted without a code change.
6. The budget is enforced: a document over budget stops and is marked partial.
7. `cargo tree -p memex-core` shows no ADK crate.
8. Token counts on Derivations match what the scripted fake reports, and a re-run through the route creates no duplicates.

## Out of scope
The review agent (025), audits (026), audio (028) and fine-tuning.

## Result
**Status: everything is built and verified except item 1's live run, which hasn't been made. No precision number exists yet, so the Goal's ≥0.9 is unproven.** No LLM was called and no key was used. Every number below comes from a scripted agent, and proves the machinery, not a model.

**Built.**
- `AgentRunner` in `memex-core` with plain types in `memex-types`, a pure one-retry policy, and `ScriptedAgent` for tests.
- `memex-agent-adk`, the only crate that depends on ADK-Rust (`=2.3.0`), wrapped in the binary by `AdkAgentRunner`. It covers Gemini and OpenAI-compatible servers, no tools and no ADK retries, and keeps a reply's tokens when ADK itself refuses it.
- The job, per rendition, is `Server::run_extraction_job`, with its pure parts in `memex_core::extraction`. It's off unless `MEMEX_LLM_PROVIDER` is set.
- Vault migration 9: `derivations` and `extractions`.
- Routes: `GET /vaults/{id}/sources/{sid}/extraction`, `POST .../extraction/rerun`, `GET /vaults/{id}/renditions/{rid}/derivations`. `JobDto` gains `waiting_since_ms`.
- Evidence now gets its anchor as it commits, which nothing did before, and item 2 needs. This applies to every claim, not only a machine's.
- `docs/architecture.md` has an Extraction section with the rules, and `AGENTS.md` the commands and settings.

**Verification.**

1. **Not run: it needs a live LLM, costs money and sends documents to a provider, so the user decides.** Precision and recall per type and language, and the live token cost, are unmeasured. Built and proved on the scripted agent:
   - The set: 50 documents (en 12, sv 13, de 13, es 12), in `crates/memexvault/tests/fixtures/extraction/`, all Wikipedia, each with its source, license and attribution: 10 CC BY-SA 4.0 at a pinned revision, 40 CC BY-SA 3.0 from the dataset `wikimedia/wikipedia`. 194 entities with 228 field values, and 190 free claims. **An agent wrote the labels and no person has checked them.**
   - The scorer has hand-made cases, and `the_harness_runs_the_whole_set_through_the_pipeline_on_a_scripted_agent` runs all 50 documents through the real pipeline with replies written from the labels and planted mistakes. Every label came back, each of the 10 planted invented quotes was refused, and each of the 10 planted unlabelled claims was left for adjudication. That output is headed as not a measurement, and its percentages say nothing about a model.
   - Estimated cost of one run, from the real prompt and documents with no call made: 59,539 input and 39,120 output tokens, about 98,700 in total, up to twice that if every call retries. The output figure assumes a model finds about twice what's labelled.
   - The exact command, once documents are approved, with a local server's base URL and `openai` in place of Gemini if preferred:
     ```
     MEMEX_LLM_PROVIDER=gemini MEMEX_LLM_MODEL=gemini-3.5-flash-lite MEMEX_LLM_API_KEY=<key> \
     MEMEX_EXTRACT_TOKEN_BUDGET=10000 MEMEX_EVAL_MAX_TOKENS=250000 \
     MEMEX_EVAL_APPROVED=crates/memexvault/tests/fixtures/extraction/all-documents.txt \
     MEMEX_EVAL_OUT=target/extraction-eval.json \
     cargo test -p memexvault --release --test extraction_eval -- --ignored --nocapture --exact live_labelled_set
     ```
   - Free-claim labels aren't exhaustive, so precision before adjudication is a lower bound. Someone must judge the `to_adjudicate` list, and rerun with `MEMEX_EVAL_ADJUDICATIONS`. A rerun calls the LLM again, so its replies may differ. Reading verdicts from the first run's output without a second call is a follow-up.
2. Passed on Postgres. `pg_extraction_every_committed_claim_resolves_to_an_anchor`: 61 committed machine claims, 0 without a resolvable anchor by SQL query. The one claim from a PDF has rectangles from the word table. Scripted replies and made-up word boxes, not a real parse.
3. Passed, in `memex-core/tests/extraction.rs`. Invented quotes: 3 results refused, 0 proposals, 0 claims, 1 call. Wrong schema: 2 calls, then the job is dead and the document `failed`, with the tokens on record. Outage: 30 rounds left the job `waiting` with 0 attempts used, while keyword search, reading and upload answered, and it finished when the agent came back. Also on Postgres in `pg_extraction_tokens_reruns_and_an_outage`, and through the worker pool in `workers.rs`.
4. Passed. Three re-runs made 0 calls, 0 new ChangeSets and 0 new Derivations, also on Postgres, with claims pending and committed. A new prompt calls again and proposes nothing the same.
5. Passed. A type added through the registry appears in the prompt and reply schema, is extracted, reviewed and filtered on, with no code change, in `a_type_added_to_the_registry_is_extracted_without_a_code_change`.
6. Passed. Over budget stops after 1 of 3 segments and is `partial` with `stop_reason: budget`. A budget too small for any call makes none, and a retry that wouldn't fit isn't made.
7. Passed. `cargo tree -p memex-core | grep -ci adk` gives 0, and the same for `memex-types`.
8. Passed. Derivations hold exactly the scripted 111/22/3, 444/55/6 and 777/88/9, and the run's sum is 1332/165/18. On Postgres, 3 calls at 500 input and 3 thinking tokens sum to 1500 and 9. A re-run through the route in `memex-http/tests/api.rs` makes 0 calls and no new row.

**Definition of done** on the merge with main at 1c8c03d, code commit `acb2a45`: `cargo build --workspace` ok, `cargo test --workspace` 535 passed, 0 failed, 58 ignored, `clippy -D warnings` ok, `fmt --check` ok, `deny check licenses` ok with no new license. Postgres tests on a fresh database: 18 passed, 0 failed, no `LeaseLost`.

Not run, as the brief says: `pg_bad_cases`, `pg_kill_postgres` (both reach beyond this worker's database), `pg_child` and `pg_job_child` (helpers), `pg_full_disk` (needs a small disk). This change adds two tables and new rows to the same commit path, and doesn't change how commits or leases work, so I expect no effect, but that's untested. `pg_kill_jobs_200_times` did pass.

**The ADK edge is tested only against a stand-in server on this machine,** for usage, truncation, empty and invalid replies, down, 503, 400, timeout and no tools offered. Not tried live: `ThinkingLevel::Minimal` on Gemini, which the spike didn't set, and the Gemini schema rewrite on registry types. A model that refuses either fails with a clear `Provider` error, so the first live call would show it cheaply.

**Decisions.**
- A proposal's idempotency key is the rendition and the fact, without prompt, model or quote, so a re-run under a new prompt can't duplicate a fact. A second mention of a fact in a document keeps the first quote.
- One proposal per result, not per batch, so a reviewer can reject one claim alone. A Derivation commits before its proposals, so a crash costs no second call.
- A model's reply gives batch-local ids (`s1`), not long object ids, so a miscopied id can't name another real segment.
- Extraction jobs are left out of the upload backlog count, as otherwise an outage would end up refusing uploads, which `docs/architecture.md` rules out. `open_job_count` takes the kinds to leave out.
- Waiting takes the attempt back and caps the delay at 5 minutes. A `Timeout` counts as an outage, so a call that always times out waits forever rather than failing. It's visible as `waiting`.
- A `failed` document is one where all batches failed. Some failing gives `partial`.
- The budget is per run. A re-run is the decision to spend more. It's checked by estimate before a call, so a provider's real count can pass it by the estimate's error, and the next call then stops.
- Extraction is one server-wide switch, with one worker. The spec doesn't ask for per-vault control, unlike chat. Anyone setting `MEMEX_LLM_PROVIDER` sends every vault's new documents to that provider.
- Free claims' values are strings, and `confidence_permille` is fixed at 1000, as a model's own confidence counts for nothing.
- The set uses opening text, not whole articles, and CC BY-SA text sits in a fixture folder with its own README, as the repository's Apache-2.0 doesn't cover it. The golden set has a similar note.
- Wikipedia's API rate-limited the fetch after 10 documents. I stopped asking it, and took the rest from the published dump, so those 40 are from 2023-11-01 and aren't pinned to a revision.

**Landed with item 1 not run, by the user's decision.** Precision and recall are still unmeasured, and spec 81 holds the live run and the label review. At landing, `pg_bad_cases` and the 8 TLS tests passed on this code. The first TLS run lost one test to its server still starting up, and one rerun passed all 8. `pg_kill_postgres`, the full-disk test and the two harness helpers weren't run.

**Follow-ups.**
- Run item 1, and have a person review the labels first and adjudicate after: spec 81.
- Reprocessing makes a new rendition, so the same fact gets a new key. Pending proposals on the old rendition stay, and a claim approved after that keeps its anchor on the old rendition. Worth settling with spec 025.
- Two mentions of one thing with different fields, across batches, give two entity proposals. Only their identity candidates link them, and only once one is committed.
- A claim identical to an active one from another document is refused by the dedup validator, instead of adding evidence to it.
- Derivations hold quotes, so erasure, when it's built, must cover `derivations`.
- A large registry makes a large prompt on every call: up to 500 types of 200 fields. The budget contains the cost, but a job would stop `partial` having done nothing. Selecting types per document is a later idea.
- Types using `$ref`, `oneOf` or objects with no properties are only approximated in Gemini's schema, and the full check still applies to the reply.
- The console has no screen for this yet.
