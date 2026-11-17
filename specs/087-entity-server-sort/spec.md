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
Built the Approach. Items 1, 2 and 4 pass on the in-memory store and on Postgres. Item 3 passes its rule on the store and facade at 200,000 entities. **Item 5 wasn't run, and neither were any `pnpm` checks:** this machine has no Node or pnpm, and installing them needs the user's say. **`cargo deny` wasn't run either,** as it isn't installed. No dependency changed. Timings come from a machine shared with five other workers, with Postgres in podman.

**What was built.**
- `GET /vaults/{id}/entities` takes `sort=<field>:<asc|desc>`, alone or with filters. `created`, or no sort, is still id order. Sorting by more than one field is refused.
- **The order.** Rows with a value come first, by value and then by id, both in the sort's direction. Rows with no value come last in both directions, by id in the sort's direction. Text sorts folded (case, diacritics and runs of space don't count), in byte order. That's the field table's own `COLLATE "C"` and Rust's `String` order. Booleans sort `false` before `true`, and numbers and dates by value. Text over 512 bytes isn't in the field table (spec 023), so it counts as no value.
- **What sorts.** A filterable field holding one text, number, boolean or date. A list has no single value to sort by, and is refused.
- **The cursor** is opaque to clients: the last row's id, a newline, then its value tagged with its kind, or `-` for no value. A cursor from another order, or a bad one, gives a 400 that says to start again.
- **Core.** `memex_types` has `FieldSort`, `SortAfter`, `SortColumn`, `FieldSort::compare` and `EntityRow::sort_value`. `memex_core::entity` has `parse_sort`, `sort_cursor` and `parse_sort_cursor`. `Server::list_entities_sorted` is the facade, and `Snapshot::entities_sorted` is new on every store. The in-memory store sorts by `FieldSort::compare`.
- **Postgres.** At most two queries in one snapshot. The first walks the sorted column's existing B-tree (`(type_name, field, <column>, entity_id)` from spec 023) from the cursor, with a row comparison as the keyset. If the page isn't full, the second lists rows with no value, by id. Filters on other fields are `EXISTS` semi-joins, so a list field with several matching values can't repeat a row. A filter on the sorted field itself bounds the index scan, and skips the second query, as a row with no value can't pass it. **No new index and no migration:** the existing ones serve it.
- **The console.** The table sends the sort to the API and shows the server's order. The "sorted within the loaded rows" message is gone, and scrolling keeps loading pages in that order. A column that can't be sorted has no sort button and says so in its title, and a line under the table names those fields. A URL sorting by a field that can't be sorted, or doesn't exist, isn't sent, and the page says why. `sortLoadedRows` is gone. `EntityColumn` has `sortable`, which mirrors `parse_sort`.
- `openapi.json` came from the server's own document (`memex_http::openapi()`, written out by a throwaway test). It differs from the committed one only in the three parameter descriptions of this route. `generated/schema.ts` was edited by hand to match, as `pnpm api:gen` couldn't run.

**Verification.**

| # | Check | Result | Measured |
|---|---|---|---|
| 1 | Sort matches an independent sort, every kind, both directions, missing values and ties | pass | `memex_core::testing::entity_sort::sort_matches_an_independent_sort`, on both stores (`a_sort_matches_an_independent_sort_of_the_same_rows`, `pg_entity_sort_matches_an_independent_sort`). 120 rows, 4 fields × 2 directions, on one page and in pages of 1, 7 and 50. Also with 7 filter sets: another field, a list field, filters on the sorted field itself, and two filters. The fixture has mixed case, diacritics, runs of space, `-0` beside `0`, negative and large numbers, dates and RFC 3339 times with offsets that tie, text over 512 bytes, and every field missing on some rows. The expected order is computed from the JSON values, not by `FieldSort::compare` |
| 2 | Every page returns each row once, on both stores, while rows are written | pass | `walking_returns_each_row_once_while_rows_are_written` on both stores: 90 rows, pages of 6, 3 new rows written after every page (each new set has values on both sides of the cursor and missing ones). No row came twice, every row there at the start came once and in order, and rows written during the walk were reached |
| 3 | 200,000 entities: sorted first page and sorted with a filter, p95 with the load. Unsorted filtering no slower | pass, on its second run | See below |
| 4 | Unknown field, unfilterable field, malformed value: clear 400 | pass | `bad_sorts_are_refused` on both stores, and `a_sort_orders_every_row_and_bad_ones_say_why` through HTTP, which checks the status and the exact message: "the type has no field nope", "the field notes isn't filterable, so it can't be sorted", "a sort is <field>:asc or <field>:desc, not 'price'", "a sort's direction is asc or desc, not 'up'", a second `sort`, and a bad cursor. Also a list ("holds a list, so it can't be sorted") and 8 malformed cursors |
| 5 | `e2e:entities` passes with a column click sorting all rows; API types regenerated | **not run** | No Node on this machine. The specs are updated (below). The document was regenerated from the server, and the types by hand |

**Item 3.** The pass rule was committed in `entity_sort_bench` (`crates/memexvault/tests/entities.rs`) before anything was measured, in `bceae54`. Through `Server::list_entities_sorted` and `list_entities`, with the console's page of 201 rows, at 200,000 entities: each lane's p95 ≤50 ms, sorted or not. Unsorted filtering runs the same query as before this spec. Rows were loaded through the store as in `entity_filter_bench`, with every tenth entity missing its price. 300 trials per lane, `--release`.

**The first run failed one lane:** a ~50% range filter on price while sorting by price, descending, had p95 **267.8 ms**. The scan walked every price above the range before it reached one that passed, testing each row with `EXISTS`. A filter on the sorted field now bounds the index scan instead. The rule wasn't changed. The second run, on a fresh database, loaded the rows in 97.8 s, 749 MB for the two entity tables with their indexes, at load about 4.5 to 5 on 16 cores:

| Lane, n=300 | p50 | p95 | p99 | max (ms) |
|---|---|---|---|---|
| Sorted first page, price asc | 5.41 | 6.88 | 8.55 | 8.93 |
| Sorted first page, price desc | 6.74 | 12.89 | 28.81 | 34.87 |
| Sorted first page, name asc | 5.02 | 6.90 | 9.73 | 12.75 |
| Sorted first page, released desc | 4.95 | 6.53 | 8.68 | 9.75 |
| Sorted, fourth page, price asc | 5.76 | 7.68 | 10.19 | 18.77 |
| Sorted with exact filter ~0.1%, price desc | 10.16 | 13.47 | 18.42 | 19.64 |
| Sorted with exact filter, 1 row, price desc | 0.60 | 0.85 | 1.13 | 1.43 |
| Sorted with range ~50%, name asc | 17.76 | 21.88 | 27.91 | 35.76 |
| Sorted with range ~50%, price desc | 5.24 | 6.47 | 7.45 | 7.77 |
| Unsorted, no filter | 4.18 | 6.33 | 7.63 | 8.45 |
| Unsorted, exact filter ~0.1% | 6.49 | 9.05 | 11.24 | 12.84 |
| Unsorted, range filter ~50% | 4.99 | 6.91 | 7.82 | 7.87 |

The first run's unsorted lanes were p95 4.58, 5.66 and 5.22 ms. Spec 045's rule is about the browser, overhead over the API's time. Its API time was p95 26.9 ms through HTTP on a debug build, and these facade times are well under that. **The browser half of item 3 wasn't run:** the sorted lanes added to `e2e-entities/scale.spec.ts` keep spec 045's rule (≤100 ms over the API's time, p95, at least 200 trials), and need Node. `memex_s087` was dropped and recreated after each run.

**Decisions.**
- **No value goes last in both directions,** so a sorted first page never has to look for missing values while most rows have one. Ties and missing rows follow the direction by id, so a descending sort is an ascending one reversed, apart from where missing values go.
- **Folded text, not the value as written,** because that's what the field table holds and its indexes serve. "Alpha" and "älpha" tie, and the id breaks the tie.
- **`sort=created` still works** as id order, since the parameter documented it, so no client breaks.
- **A known limit:** sorting by one field while filtering on another, where the two are strongly correlated, can make the scan read many rows before it finds a page. The lanes above don't hit that, and the planner drives selective filters from their own index (the 1-row lane: 0.85 ms). A field-specific index for such pairs would be a new spec.
- The concurrency check writes new rows, as the spec says. It doesn't change rows' values during a walk. A row whose value changes between pages can move past the cursor, which is what keyset paging does.

**Checks.** On this branch, with the pdfium workaround below:
- `cargo fmt --check` and `cargo clippy --workspace --all-targets -- -D warnings`: pass.
- `cargo test --workspace --no-fail-fast`: 987 passed, 112 ignored. Ten tests fail, all because no pdfium is bound: `memex-pdf`'s 8 `fixtures` tests, and `pdf_pipeline`'s `a_pdf_becomes_a_rendition_with_segments_and_identical_bytes_when_parsed_again` and `bad_pdfs_fail_one_job_each_and_the_next_job_still_runs`. All of them parse a PDF, and none touches entities.
- `pg_` entity tests on Postgres: 5 of 5 (`pg_entity_*`, including spec 023's two).
- `cargo deny check licenses`: not run, as it isn't installed. No dependency was added.
- `pnpm` checks and `e2e:entities`: not run, as there's no Node.

**pdfium.** `scripts/fetch-pdfium.sh` pins only a macOS build, and `memex-pdf` turns on pdfium-render's `core_graphics` feature, which is Apple-only. So `memexvault` doesn't compile on this Linux machine as committed. To build and run its tests here, I turned off `static`, `libc++` and `core_graphics` and bound pdfium at run time, **locally and never committed**. The PDF tests can't pass that way, and aren't this spec's.

**e2e changes, not run.**
- `entities.spec.ts`: a column click is checked against the API's own order, a column that can't be sorted says so, and a URL sorting by one is refused.
- `scale.spec.ts`: a column click on 200,001 rows must put the highest price in the database first, though only 200 rows are loaded. Five sorted lanes are held to spec 045's rule. The old loaded-rows sort timing is gone.
- The table's status gained `data-sort-key`.

**Follow-ups.**
- Run `pnpm` typecheck, lint, test, build and `e2e:entities`, with its scale check, on a machine with Node. Then run `pnpm api:check` to confirm the hand edit of `schema.ts`.
- Run `cargo deny check licenses`.
- A Linux pdfium pin, so `memexvault` builds here.
- Spec 045's follow-up asked for "a total if it's cheap". Not done: it isn't cheap with filters at 200,000 rows, and it isn't in this spec's Approach.
