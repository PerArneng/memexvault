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
Not started.
