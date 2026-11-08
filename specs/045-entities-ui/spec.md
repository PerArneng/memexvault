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
Built everything in the Approach, with **one narrowing the user can overrule: sort covers only the rows loaded so far** (below). All seven Verification items pass. Timings are from the final run, on a debug server and Chromium, on a machine shared with other workers, so the load average is beside each. No file under `crates/` changed.

**What was built.** The Entities section, under `web/apps/console/src/entities/`, loaded lazily as a 13.4 kB gzip chunk (55 kB raw). `pnpm budget` is unchanged in kind: the sign-in route is 126 kB gzip against 200 kB. Pure parts are in `packages/core`:
- `entity-schema.ts`: `schemaView` is the function from a schema to fields, columns and typed filters, and the only code that reads a schema, so a new type needs no frontend change. It mirrors the server's `fields_of` and `parse_filter`.
- `entity-draft.ts`: the form's draft and the schema it writes, which keep rules the form doesn't show. Also the approval wording, the merge and split previews and the graph.
- `entity-routes.ts`, `entity-cases.ts`, `testing-entities.ts`, and `translate-entities.ts` in `packages/api`.

The screen has a type list, versions and JSON Schema, a schema form with `searchable`, `filterable` and `identity`, and a preview that must be asked for before Submit, and again after any edit. The table has server-side typed filters, column choice, loaded-row sort and search through the resolve route, all in the URL. An entity shows each field's value, the Evidence link into spec 42's viewer, the current choice and why, and conflicting values with their own sources. It also shows aliases, merged-from and redirects. Create, edit, replace, merge and split are proposals, `same_as` suggestions lead to a merge preview, and pending proposals link to spec 46's route. The graph panel lists and draws nearby objects.

**Verification.**

| # | Check | Result | Measured |
|---|---|---|---|
| 1 | New type usable and searchable within the lag target, no frontend change or reload | pass | See below: p95 452 ms against 1 s, n=40, load about 13 |
| 2 | Each field value links to a highlighted region | pass | 3 of 3 fields on a real PDF (`clean_pages.pdf`), each clicked through to the viewer with a non-empty `[data-highlight]`, load 14 to 25. How exactly a highlight lines up is spec 42's item 1, not rechecked here |
| 3 | Conflicts show together, with the choice and why | pass | Both values, "Current choice" and "Not chosen", each with its own evidence, and the server's own reason: "both claims have the same standing, and this one is newer" |
| 4 | A breaking change and a merge reach the queue, not applied | pass | Both changesets found in `GET /review/queue?state=pending`. The type stayed at version 1 with its field, and the merged entity stayed itself until the merge was approved through the API. A split and an agent's new entity wait too. Spec 46's screen isn't on this branch, so its links were checked by address, not followed |
| 5 | Schema-to-columns tests | pass | `entity-schema.test.ts`: 11 field shapes plus those with no comparable value, 11 non-schemas, wrong shapes inside a schema, `__proto__` as a field, 100,000 fields, a 100,000-value enum, and 10,000 levels of nesting |
| 6 | 200,000 entities filter and scroll without freezing, filter p95 ≤100 ms over the API's time | pass | See below: 29.8 ms over the API, longest task under 50 ms, load 12 to 15 |
| 7 | HTML in values renders as plain text | pass | A value with `img`, `script` and `b` in the table, detail, aliases, conflicts, merge and split previews, search results and a schema description: shown literally, no such element in `main`, nothing ran, no CSP error |

**Item 1.** The lag target, quoted: spec 018, "indexing lag has p95 ≤1 s", and spec 023's item 1, "target ≤1 s". Measured from the proposal's answer in the browser to `search/lexical` returning the entity, polling as fast as the API answers, through the UI on 5 brand-new types. The column, filter and Find were then used without a reload, checked by a marker on `window`.

| Run | n | p50 | p95 | max | Load | Result |
|---|---|---|---|---|---|---|
| First | 20 | 265 | 1068 | 1068 ms | about 290 | **miss** |
| Second | 40 | 394 | 496 | 513 ms | about 82 | pass |
| Third | 40 | 395 | 451 | 469 ms | about 15 to 25 | pass |
| Final | 40 | 422 | 452 | 455 ms | about 13 | pass |

The first run's p95 was its slowest single trial. I then raised n to spec 023's 40. The miss is kept here as what that load does.

**Item 6.** The pass rule was committed in `e2e-entities/scale.spec.ts` before anything was measured:
- Filter: from the form's submit to the rows painted, less that request's Resource Timing, p95 ≤100 ms over at least 200 trials, with a new value each time so no cache answers.
- Scroll: first row to last with every page loaded, longest task ≤250 ms, p99 frame gap ≤100 ms, every row reached, at most 200 row elements in the DOM.

Rows went straight into the tables with `generate_series`, the same shortcut as spec 023's `entity_filter_bench`. 200,001 entities and 1,000,002 field values took 25 s at load about 12 to 15, and came to **335 MB + 415 MB per load**. Three runs left 2,318 MB in `memex_s045`, which has been dropped and recreated, and it's now 35 MB. These rows aren't in the event log or the search index, so only the filter and list routes were measured against them.

| Filter lane, n=40 each | Overhead p50 | p95 | Rows on the first page |
|---|---|---|---|
| Exact, one entity | 22.4 | 35.6 ms | 1 |
| Exact, about 0.1% | 23.9 | 26.8 ms | 200 |
| Range, about 0.1% | 23.6 | 26.8 ms | 180 to 200 |
| Range, about 50% | 24.3 | 28.0 ms | 200 |
| Exact and range | 23.5 | 30.3 ms | 100 |
| **All, n=200** | **23.5** | **29.8 ms** | |

- The API's own time in those trials: p50 20.3, p95 26.9, max 32.1 ms, on debug. Load 13 to 15.
- Earlier runs on the same rule: p95 33.9 ms at load about 40, and 34.4 ms at about 18, when the API's max reached 201.6 ms.
- Scroll: 200,001 rows in 52 s, frame gap p50 16.7, p95 19.5, p99 20.8, max 24.5 ms, **no long tasks at all** (the browser reports those over 50 ms), 44 row elements at most, a 133 MB heap. Load 12 to 13. An earlier run at load about 40 had one 51 ms task.
- Outside the rule: sorting all 200,001 loaded rows took 74 ms, and filtering with them all loaded had overhead p95 26.1 ms (n=20).

**Decisions.**
- **Sort is narrowed from the spec's wording, by agreement with the coordinator, and the user can overrule it.** `GET /entities` lists by id only, and its `sort` accepts only `created`. A column click sorts only the rows loaded so far, and the page says so at once: "Sorted by name within the 3 rows loaded so far, not across every row." Automatic loading stops while sorted, so rows don't move, and there's a "Load more rows" button. Filters always run on the server across every row.
- **Filters apply on a button or Enter,** not per keystroke, so there's no debounce in the response time and no request per letter.
- **There's no total.** The API gives none, so the table says how many rows are loaded and whether more exist.
- **The URL.** TanStack Router rewrites a repeated query parameter as a JSON array and reads a value that looks like a number as one. So all filters are one `f` value, a line each, a merge's target is in the path as an id could look like a number, and `find` is read back from the router's quoting. Other screens with repeated parameters will meet this.
- **The graph asks about the entity and its claims,** up to 24, as an entity's links are on its claims. It shows which field each link came through, and says when a lookup failed or the list was cut.
- **Evidence is entered as a segment id and a quote,** which is what the server checks. There's no picker yet.
- **`commands.ts`:** the palette opened a type at `/entities/<name>`, now an entity's address, so it opens the type's table. `DataTable` gained optional `onNearEnd`, `headerCell` and `ariaSort`.
- **A saved edit's notice** lives above the form, as a new version restarts the form.
- **No new modal,** so no new CSP errors: 0 from this screen. The palette's 3 from spec 40 are unchanged.
- **e2e setup retries a 503.** During these runs other workers' servers held all 64 Postgres connections. A POST can answer 503 after landing, so retried names change.

**Definition of done,** at load 8 to 30, unless noted. Chromium only, as that's the setup.
- `pnpm typecheck`, `lint`, `test` (455), `run licenses` (396 packages), `run contrast` (53 pairs, 0 failed), `budget`, `e2e` (gallery, 6): pass.
- `e2e:entities`: 2 of 2 scale tests, and 13 of 13 others. `e2e:shell`: 17 of 17. `e2e:wiki`: 17 of 17.
- `e2e:documents`: 9 passed, 1 skipped (the player, with no `MEMEX_E2E_RECORDING`), **1 failed: its item 5, 1,849 ms against 1,000 ms, on a debug server.** Spec 042's Result already says "A debug build misses item 5" (2,308 ms there) and passes it on release. This spec changed nothing in the viewer, and I didn't make a release build to confirm it, as the brief asks to avoid those.
- `cargo build -p memexvault`: pass. The other cargo checks and the Postgres tests weren't run, as no file under `crates/` changed. `git diff 2cf6493 -- crates` is empty.
- `pdfium` came from the main checkout's `vendor/`, the same pinned build, as `scripts/fetch-pdfium.sh` got 504s from GitHub.

**Not exercised end to end.** The `contradicts` and `wiki_link` kinds in the graph panel (only `claim_ref`, with the merge of links in unit tests), an approved split, and a recording's evidence link, which is spec 42's. No LLM or paid call was made.

**Follow-ups.**
- A server-side `sort=<field>:<asc|desc>` on `GET /entities`, with a total if it's cheap. The sort narrowing above ends there.
- An evidence picker instead of typing a segment id.
- `same_as` suggestions can't be dismissed, as spec 023 noted.
- Whether other screens should share the URL rules above.
