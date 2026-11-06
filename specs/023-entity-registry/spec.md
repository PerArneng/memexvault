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
Not started.
