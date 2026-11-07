---
id: 023
name: entity-registry
description: Add runtime-defined entity types with searchable, traceable fields and reversible merges
---

# 023 entity-registry

## Goal
Structured entities such as Person, Product and Project, plus types defined at runtime. Each field traces to evidence, and a
new type becomes searchable without a restart or reindex.

## Background
**Depends on:** 013, 018. Design: `docs/architecture.md`, section Dynamic entity types.

## Approach
- EntityType as a versioned JSON Schema, using `jsonschema` with remote `$ref` disabled. Annotations: `searchable`,
  `filterable`, `identity`.
- Seed types: Person, Product, Project, Organization, Decision.
- Type changes through ChangeSets. Additive versions auto-validate, and breaking ones need human approval.
- Field values are Assertions, with current state materialized from them. Conflicting values stay visible.
- Search through a tantivy JSON field, a rendered text card sent to embedding, and a Postgres field-value table for exact and range filters.
- A resolve API from mention to candidates, with aliases and identity fields.
- `same_as` candidates only as suggestions. Merge and split are human-approved and reversible, with redirects.
- Typed filter API, such as `type=Product, price<100`.
- Routes for type list, version history and a preview classifying a change as additive or breaking. Entity list by type, get, and create and edit as proposals. Merge and split proposals, the `same_as` suggestion list, and redirect lookup.

## Verification
1. A brand-new type with entities is found by text, exact and filtered search within the lag target, without restart.
2. Each field value in a returned entity links to Evidence that resolves to a source region.
3. Two sources with different values both appear, along with the current choice and why.
4. A schema evolution test: additive versions keep old entities valid, and breaking versions wait for approval.
5. A merge and its revert restore the materialized state, and old ids redirect during the merge.
6. Exact filter p95 ≤2 ms at 200k field values, with a projection to 1M.
7. Bad cases: invalid schema, remote `$ref`, values failing validation, a type with 10k distinct fields (refused by a limit),
   and a merge proposed by an agent never auto-commits.
8. The preview's additive or breaking result matches what the type change does when it's submitted.

## Out of scope
Automatic merges, agent-created types going live without review, ontology import and RDF export.

## Result
Built everything in the Approach, and **item 6 misses its target** as measured: exact filter p95 is 2.55 to 4.02 ms at 200k values on a shared machine, against ≤2 ms. The rest of the Verification passes. Timings were all taken on a machine shared with other workers, with Postgres behind Docker's port forwarding.

**Landed with item 6 unmet, by the user's decision.** The target stays in `docs/architecture.md`, and spec 062 reviews it. The user also accepted allowing MIT-0 in `deny.toml`.

**What was built.** `memex_core::entity` holds the type registry and pure rules: schema checks, additive or breaking classification, seed types, the choice of current value, the text card, typed filters and planning. New `Op`s (`DefineEntityType`, `CreateEntity`, `SetEntityAliases`, `MergeEntities`, `SplitEntity`) go through the normal proposal flow, with one new validator, `entity`. Vault migration 7 adds `entity_types`, `entities` and `entity_fields`. `DocKind::Entity` and `ItemKind::Entity` carry entities through both projectors, the fence, `as_of` and the evidence package. The tantivy schema gains an `attrs` JSON field. `crates/memex-http/src/routes/entities.rs` serves 11 routes under `/vaults/{id}/entity-types` and `/vaults/{id}/entities`.

**Verification.**

| # | Check | Result | Measured |
|---|---|---|---|
| 1 | New type found by text, exact and filtered search within the lag target, no restart | pass | `pg_entity_new_type_is_found_within_the_lag_target_without_a_restart`, real Postgres, tantivy and pgvector with a fake embedder: commit to keyword hit p50 542 ms, p95 566 ms, max 581 ms, n=40, target ≤1 s. Store filters answer at once. 40 cards embedded and in the vector index 737 ms after their jobs started |
| 2 | Each field value links to Evidence that resolves to a region | pass | `every_field_of_a_returned_entity_leads_to_a_source_region`: lineage with rectangles and `highlight` for every field. The fixture is a synthetic parsed PDF, not a real file |
| 3 | Two sources both appear, with the choice and why | pass | `two_sources_disagree_and_both_show_with_the_choice_and_why` and `conflicting_values_stay_visible_with_the_choice_and_why` |
| 4 | Additive keeps old entities valid, breaking waits | pass | `additive_versions_keep_old_entities_valid_and_breaking_ones_wait` |
| 5 | Merge and revert restore state, old ids redirect | pass | Rows compared exactly, on the fake and on Postgres, including what filters read |
| 6 | Exact filter p95 ≤2 ms at 200k, projection to 1M | **fail** | See below |
| 7 | Bad cases | pass | Invalid schema, remote and file `$ref`, failing values, 10k fields refused by the limit of 200, and an agent's merge stays pending with auto-commit earned or not |
| 8 | Preview matches submission | pass | `the_preview_matches_what_a_submission_does`: 7 cases. Both run `preview_change` and the risk table |

**Item 6.** `entity_filter_bench`, 2,000 queries per lane, rows loaded directly. Final run on `641d77e`, load average 4 to 6, p50 / p95 in ms:

| Lane | 50k values | 200k values |
|---|---|---|
| Control: open and close a snapshot, no query | 0.76 / 1.33 | 0.78 / 1.51 |
| Control: one row by primary key | 1.23 / 2.17 | 1.27 / 2.38 |
| Exact, unique value, 1 row | 1.26 / 2.06 | 1.31 / 2.55 |
| Exact, ~0.1% of entities, up to 20 rows | 1.69 / 3.07 | 1.99 / 4.02 |
| Range, ~0.1% | 1.56 / 2.74 | 1.99 / 3.68 |
| Range, ~50% | 2.13 / 4.06 | 2.06 / 3.93 |
| Exact and range | 1.49 / 2.77 | 2.16 / 4.13 |
| Exact query alone, in an open snapshot | 0.76 / 1.48 | 1.16 / 2.19 |
| Exact through `Server::list_entities` | 2.16 / 4.68 | 2.44 / 4.72 |

The p95 varied between runs: exact on one value was 2.04 to 2.88 ms at 200k, and on ~0.1% of entities 2.54 to 5.30 ms, at load averages from 4 to 66. **Projection to 1M:** measured rather than projected in one run of the three-size benchmark, under the heavier load of that run: exact unique p95 2.65 ms, ~0.1% 4.15 ms, against 2.54 and 5.30 at 200k in the same run. So the size of the table isn't what moves it, and the indexes hold at 1M, at 419 MB for the field table.

Reading of it: a primary-key read on this setup already misses 2 ms, and a snapshot alone takes three quarters of the budget or more (p95 1.33 to 1.87 ms across runs), and spec 020 also found exact lookups over target behind Docker. Exact filters on one value cost about what a primary-key read does. Returning 20 rows costs more, in decoding JSONB. I didn't change the snapshot, since that's outside this spec, so I haven't shown the target can be met. Spec 062 reviews latency, and a run on a quiet machine or native Postgres is the check still owed.

**Rerun after all workers finished.** One run at 200k on a fresh database, with the pass rule set beforehand (both exact lanes ≤2 ms), load average 7 to 9, so still not a quiet machine: exact unique p95 2.28 ms, ~0.1% of entities 3.34 ms, primary-key read control 2.36 ms, snapshot control 1.64 ms. Item 6 still fails.

**Landing checks,** merged with specs 59 and 37: workspace build, tests, clippy, fmt and the licence check, the selected Postgres tests, `pg_bad_cases` alone on a fresh database, and the 8 TLS tests all passed.

**Not run.** `pg_kill_postgres`, `pg_child` and `pg_job_child` (helpers run by their harness) and `pg_full_disk_returns_an_error_and_data_stays_readable` (needs a small disk). The change touches the commit path these cover only by adding a table.

**Decisions.**
- **Field values** are Assertions with the subject `entity:<id>`, and predicate the field name. They're indexed as claims as well, so a search can return an entity and the claims behind it. Claim text shows that raw subject.
- **Current value:** epistemic status, then a person over a machine, then newest. Confidence and trust tier play no part: a model reports its own confidence, and a tier can change later with nothing to re-materialize.
- **A person's value may have no evidence,** as `validate_evidence_required` already allows, and then its field shows an empty list. Item 2 holds for every value with evidence, and for every machine value.
- **Seed types are built in as version 1,** not written to each vault's log, so vault creation still writes no events and the log's first event is still someone's own. They aren't in the `entity_types` table until they get a version 2.
- **The tantivy JSON field** is queried through a typed scope, `attr=<type>.<field>`, since there's no query syntax to name a path in. It's keyword only, and `as_of` refuses it. The schema version moved to 3, so **each existing vault's keyword index rebuilds once on upgrade**. New types need no rebuild after that.
- **Filters** are `filter=<field>:<op>:<value>`, repeated, rather than `price<100`, as a query DSL is ruled out. Every scalar goes in the field table, so a new `filterable` annotation needs no backfill. Text over 512 bytes isn't there, as a B-tree entry has a size limit.
- **Limits:** 200 fields per type, counting nested ones, 500 types per vault, 256 KB per schema, 50 aliases per entity.
- **An agent can't approve or revert** a merge, a split or a breaking change, or approve a type an agent proposed, whatever its scope. This is new in `approve` and `revert`. Other always-review kinds, such as deletes, still only check scope, so an agent with admin scope can approve those. Spec 025 should close that.
- **A revert** needs every touched row to be as it was left, so a later edit to the target blocks reverting a merge until that edit is reverted first. A type's revert also refuses while entities use that version or a newer version exists.
- **An approved breaking change** leaves old entities as they are, at the `type_version` they were checked against. Their next edit has to satisfy the new schema.
- **`same_as` suggestions** are computed on read and never stored, so there's no way to dismiss one yet.
- **Search:** `is_object_id` only accepted UUIDs while the server generates 32 hex digits, so the exact path never fired for real ids. It accepts both now. Entity lookup adds one store call to every hybrid search with no `attr`.
- **`deny.toml`** now allows MIT-0, for `borrow-or-share` through `jsonschema`. `pg_contract`'s cleanup list gained two vaults, and a keyword index test no longer fails on temp-dir cleanup racing tantivy's merges, which failed 2 of 25 runs on this branch. I didn't check the rate on `main`.

**Follow-ups.** Meet or revise the exact filter target (spec 062). Show entity names in claim text. Entity-valued fields don't follow redirects by themselves, so callers use the redirect route. `valid_from` and `valid_to` don't yet affect the current value. Migrating old entities after a breaking change is by hand, one edit at a time.
