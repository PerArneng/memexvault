---
id: 045
name: entities-ui
description: Build entity types, entity tables and traceable fields with conflict handling
---

# 045 entities-ui

## Goal
A user can define a new entity type and browse and filter its entities with no frontend change, and can trace every field value to its source.

## Background
**Depends on:** 023, 042. Design: `docs/architecture.md`, section Dynamic entity types.

## Approach
- Type list, versions, and a form-based schema editor with field annotations (`searchable`, `filterable`, `identity`). A preview shows whether a change is additive or breaking, and what approval it needs.
- Tables generated from the type's schema, with typed filters for filterable fields, sort and column choice. A pure function maps a schema to columns and filters.
- Entity detail: each field with its current value, Evidence link and any conflicting values with sources. Aliases and redirects.
- Create and edit as proposals. `same_as` suggestions, and merge and split proposals with a clear preview. They always go to review.
- A graph panel for an entity's nearby objects through the neighborhood API.

## Verification
1. A new type created in the UI is usable and searchable within the lag target, with no frontend change or reload.
2. Each field value on a fixture entity links to a highlighted source region.
3. Conflicting values show together, with the current choice and why.
4. A breaking type change and a merge both end up in the review queue, not applied.
5. The schema-to-columns function has unit tests for all field types and for malformed or very large schemas.
6. A table with 200,000 entities filters and scrolls without freezing, with filter response p95 ≤100 ms over the API's time.
7. Field values containing HTML render as plain text.

## Out of scope
Automatic merges, ontology import, and bulk editing outside proposals.

## Result
Not started.
