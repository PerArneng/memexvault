---
id: 087
name: entity-server-sort
description: Sort an entity table on the server by a field, across all its rows
---

# 087 entity-server-sort

**Depends on:** 023, 045.

## Goal
A person can sort an entity table by a field across all its rows, with results on the first page, and scrolling keeps that order.

## Background
`GET /vaults/{id}/entities` returns rows in id order, and its `sort` parameter accepts only `created`, which is ignored. Spec 045 asked for sort and, with the user's agreement, shipped sorting among the rows already loaded, which the page says plainly, because a true sort needs a backend change that also touches the filter query. Spec 045 measured filtering at 200,000 rows, loaded by SQL.

## Approach
- `sort=<field>:<asc|desc>` on the list route, for filterable fields, with id as a tiebreak so the order is stable and the cursor stays correct.
- Rows with no value for that field have one stated place, in both directions.
- It works together with filters, on the indexes spec 023 already maintains if they serve it. Any new index is a migration with its cost measured.
- Spec 045's table uses it and drops the "sorted within the loaded rows" message. Fields that can't be sorted say so.

## Verification
1. For each field type, a sort over a fixture matches an independent sort of the same rows, in both directions, including missing values and ties.
2. Walking every page with a sort returns each row exactly once, on Postgres and the in-memory store, including while new rows are written.
3. At 200,000 entities, with the pass rule set before measuring, sorted first page and sorted with a filter have their p95 recorded with the load, and unsorted filtering is no slower than spec 045's numbers by the same rule.
4. An unknown field, a field that isn't filterable and a malformed value give a clear 400.
5. Spec 045's `e2e:entities` passes with a column click sorting all rows, and the API types are regenerated.

## Out of scope
Sorting by more than one field, and sorting outside entity tables.

## Result
(Filled in when the spec moves to done or dropped: what was built, decisions, numbers, follow-ups.)
