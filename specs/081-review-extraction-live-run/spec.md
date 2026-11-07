---
id: 081
name: review-extraction-live-run
description: Run the live LLM checks owed by specs 024, 025 and 029, and record precision, recall and agent results
---

# 081 review-extraction-live-run

**Depends on:** 024, 025, 026, 029, 049.

## Goal
Find out whether extraction meets spec 024's goal of precision ≥0.9, whether the reviewer meets spec 025's precision of ≥0.9 on approvals and its injection goal against a real model, whether a real agent can answer from spec 029's MCP tools, and whether spec 049's chat agent cites what it says. Those specs landed with their live runs not done, so none of these numbers exist yet.

## Background
Spec 024's item 1 needs a live LLM run, which costs money and sends documents to a provider, so it needs the user. The user chose to land spec 024 first. Its other items ran only on a scripted agent, and the ADK edge has run only against a local stand-in server, so Gemini's settings are untried live.

Spec 029's item 5 needs an agent to answer 10 golden questions through MCP: about 80,000 tokens, capped at 150,000. Its pass rule is set: all 10 cite only sources the tools returned, and at least 8 cite the right one. Spec 025's items 1 and 2 are about 55,400 and 13,300 tokens by estimate, and the commands are in its Result. An agent wrote spec 025's 48 reviewer labels too, and the report says `NOT A MEASUREMENT` until a person has checked them all.

Spec 026's item 2 needs a real model on its planted errors, as the LLM-only kinds caught 0 of 32 without one and its 80% target stays unjudged. `live_planted_recall` makes about 208 calls, with about 136,000 input tokens and at most about 426,000 output, plus any thinking tokens. It sends only the fixture's made-up sentences, so it needs no approval to send documents. Its false-positive rate should also inform whether one failure should keep revoking auto-commit.

The labelled set is 50 public Wikipedia documents in `crates/memexvault/tests/fixtures/extraction/`. An agent wrote the labels and no person has checked them. Free-claim labels aren't exhaustive, so precision before adjudication is only a lower bound.

Spec 049's item 1 needs a live model on 20 golden questions, with every factual sentence citing a resolvable anchor or the reply saying it found nothing. It's 150,000 to 300,000 tokens by estimate, capped at 400,000, it sends the questions and public Wikipedia passages, and the command is in spec 049's Result. The same run should show whether injection fixtures move a real model in chat, as spec 049's item 2 proved only that the tools can't act on it. On OpenAI-compatible servers ADK sends the agent's instruction with role `user`, not `system`, so compare that with Gemini if both are run.

## Approach
Ask the user for:
- An API key, and approval to send the 50 documents to the provider. The estimate is about 98,700 tokens, up to about twice that with retries, and the run stops at 250,000.
- A review of both sets of labels before the runs, and a ruling on the `to_adjudicate` list after extraction's run.

Then run the live commands from the Results of specs 024, 025 and 029, and rerun extraction with the rulings, which calls the LLM again. Without spec 026's run they're about 247,000 tokens by estimate, up to about twice that with retries.

## Verification
1. Extraction precision and recall are recorded per type and per language, with token cost, after a person's adjudication.
2. Reviewer agreement against human labels, and the injection fixtures' results on a real model, are recorded.
3. The MCP agent test is scored by its pass rule.
4. Spec 026's planted-error recall per kind, overall against 80%, and its false-positive rate, are recorded on a real model.
5. Spec 049's citation result on 20 golden questions is recorded, with token cost.
6. Precision ≥0.9 keeps the design. If anything misses, the user decides what follows.

## Out of scope
Changing prompts or models before the first measurement. A per-vault provider switch, which is a separate follow-up.

## Result
(Filled in when the spec moves to done or dropped: what was built, decisions, numbers, follow-ups.)
