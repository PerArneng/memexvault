---
id: 081
name: review-extraction-live-run
description: Measure extraction precision and recall with a live LLM run on the labelled set
---

# 081 review-extraction-live-run

**Depends on:** 024.

## Goal
Find out whether extraction meets spec 024's goal of precision ≥0.9. Spec 024 landed with its live run not done, so no precision or recall number exists yet.

## Background
Spec 024's item 1 needs a live LLM run, which costs money and sends documents to a provider, so it needs the user. The user chose to land spec 024 first. Its other items ran only on a scripted agent, and the ADK edge has run only against a local stand-in server, so Gemini's settings are untried live.

The labelled set is 50 public Wikipedia documents in `crates/memexvault/tests/fixtures/extraction/`. An agent wrote the labels and no person has checked them. Free-claim labels aren't exhaustive, so precision before adjudication is only a lower bound.

## Approach
Ask the user for:
- An API key, and approval to send the 50 documents to the provider. The estimate is about 98,700 tokens, up to about twice that with retries, and the run stops at 250,000.
- A review of the labels before the run, and a ruling on the `to_adjudicate` list after it.

Then run the `live_labelled_set` command from spec 024's Result, and rerun with the rulings, which calls the LLM again.

## Verification
1. Precision and recall are recorded per type and per language, with token cost, after a person's adjudication.
2. Precision ≥0.9 keeps the design. If it misses, the user decides what follows.

## Out of scope
Changing prompts or models before the first measurement. A per-vault provider switch, which is a separate follow-up.

## Result
(Filled in when the spec moves to done or dropped: what was built, decisions, numbers, follow-ups.)
