---
id: 022
name: pages-links
description: Add wiki pages with blocks, backlinks, history and claim references, searchable and traceable
---

# 022 pages-links

## Goal
Wiki-like pages that agents and people can create and edit: stable ids, typed links, backlinks and full history. Claims are
embedded by reference, so a fact never drifts between pages.

## Background
**Depends on:** 013, 018. Design: `docs/architecture.md`, sections Data model, Search.

## Approach
- Page, Block and Revision objects, with Markdown as the import, export and agent-editing format.
- A pure parser for `[[wiki links]]` and claim references such as `{{claim:<id>}}`. Links become Edges with the block as
  evidence, so backlinks are a query.
- Slugs with redirects: a rename keeps every link working.
- Writes go through ChangeSets with optimistic versions. Human and agent policies differ through the risk function.
- API: create, read at a revision, update, diff, list backlinks, list unresolved links.
- Pages and blocks are added to the lexical and vector projectors. Search adds k-hop expansion over Edges for relational
  queries, with its gain measured.
- Routes for page list, revision list, delete and rename as ChangeSets, revert, outgoing links and redirect lookup, and Markdown import and export.

## Verification
1. Renaming a page keeps all links and backlinks correct, verified on a generated graph of 10k pages.
2. An edit with a stale version returns a conflict, with no lost update under 100 concurrent editors.
3. Any past revision can be read, and a revert restores it exactly.
4. A page claim reference shows the current claim and its citations, and a superseded claim is marked as such.
5. Markdown round-trip is stable on fixtures: parse, render, parse gives the same blocks.
6. A new page is searchable within the lag target. k-hop expansion ships enabled only if it improves golden-set relational
   questions, with the numbers recorded.
7. Bad cases: broken links are listed and not fatal, link cycles terminate, and oversized pages are refused.
8. Export followed by import reproduces the page content and links.

**How item 6's k-hop half is decided.** `golden.jsonl` has no relational questions and its corpus has no links, so the coordinator decided on a separate fixture, `crates/memexvault/tests/fixtures/golden/golden_relational.jsonl`, leaving `golden.jsonl` untouched. It holds about 40 two-hop questions, written by a helper with no access to the search code. Each question matches passage A, its answer is only in passage B, and pages for A and B are joined by a wiki link built for the test, with distractor links. The fixture refers to passages by line and hash, and holds only short question and answer text.

Fixed before the first measurement, and not to be changed after seeing numbers. k-hop ships enabled only if all three hold, measured on and off in the same run:
- relational recall@20 rises by at least 10 points with k-hop on,
- recall on the existing 230 questions doesn't drop, and
- hybrid p95 doesn't rise by more than 10%.

If it passes, it runs by default only on queries a deterministic rule classes as relational. If it misses any of the three, it ships off and the numbers are recorded. Parameters, also fixed first: 1 hop, the top 5 fused candidates as seeds, at most 8 neighbouring pages per seed, at most 10 added candidates, and a neighbour scores half its seed's fused score.

## Out of scope
Live co-editing, page permissions and page templates.

## Result
Built everything in the Approach. **k-hop expansion ships off**, because it missed one of the three conditions fixed before measuring: p95 rose 18.5% against a limit of 10%. The rest of the Verification passes. Timings were taken on a machine shared with other work, with Postgres behind Docker's port forwarding.

**What was built.** `memex_core::page` holds the Markdown parser, slugs, block identity across edits, planning and search documents. `Op` gains `RenamePage` and `RevertPage`, and `CreatePage` gains an optional slug and old slugs. There's one new validator, `page`. Vault migration 8 adds `blocks`, `page_slugs` and `page_revisions`, slug columns on `pages`, and `block_id` on `edges`. `DocKind` and `ItemKind` gain `Page` and `Block`, carried through both projectors, embed jobs, the fence, `as_of` and the evidence package. `search::expand` is the k-hop step, with `expand=auto|on|off` on the search API. `crates/memex-http/src/routes/pages.rs` serves 15 operations under `/vaults/{id}/pages`.

**Verification.**

| # | Check | Result | Measured |
|---|---|---|---|
| 1 | A rename keeps links and backlinks on a 10k graph | pass | `pg_page_rename_keeps_links_on_a_generated_graph_of_10k_pages` and its in-memory twin. Two renames wrote one event each, whatever linked to them. The hub's 1,000 backlinks were unchanged, with no broken links. Writing the graph took 48.5 s, and renaming the hub 15.8 ms |
| 2 | A stale edit conflicts, and no update is lost under 100 editors | pass | `pg_page_no_update_is_lost_under_100_concurrent_editors`: revision 101, all 100 lines kept, 4,950 stale retries, 17.0 s. Without retries, 1 of 100 got through and 99 got a conflict. At most 16 editors read at once: see Findings |
| 3 | Any revision reads, and a revert restores it exactly | pass | Every revision's body, title and slug read back. The revert gave the same body, block ids, kinds, text and links, as a new revision |
| 4 | A claim reference shows the current claim and citations, and marks a superseded one | pass | Current, superseded, a chain of two, and missing, with evidence. The page was written once, and its revision didn't move |
| 5 | Markdown round-trips to the same blocks | pass | 18 fixtures and 3,000 generated documents: parse, render, parse gives the same blocks |
| 6a | A new page is searchable within the lag target | pass | `pg_page_new_pages_are_found_within_the_lag_target`: commit to keyword hit p50 529 ms, p95 562 ms, max 577 ms, n=40, target ≤1 s. 40 pages embedded and in the vector index in 481 ms with a fake embedder |
| 6b | k-hop ships enabled only with a measured gain | **misses, so it ships off** | See below |
| 7 | Bad cases | pass | Broken links listed and not fatal. Cycles end at 1, 2, 3 and 50 hops. A page over 256 KB is refused with nothing written. Too many blocks or links, and bad titles and slugs, are rejected |
| 8 | Export then import reproduces content and links | pass | Into another vault, in an order that leaves links waiting: the same body, blocks, slugs, old slugs, links and backlinks, and the same exported file. Also on Postgres |

**k-hop.** `relational_khop`, all arms in one run on the same vault, on commit `a73097a`, one run as measured, load average about 15 to 60 (a busy machine). Later commits don't touch the search path. Output is in `khop-relational.txt`.

| Condition, fixed before measuring | Off | On | Verdict |
|---|---|---|---|
| Relational recall@20 rises by at least 10 points | 20.0% (8/40) | 65.0% (26/40), +45.0 | met |
| Recall on the existing set doesn't drop | 97.5% (195/200) | 97.5% (195/200) | met |
| Hybrid p95 rises by at most 10% | 27.3 ms | 32.4 ms, +18.5% | **missed** |

p50 was 21.5 ms off and 21.6 ms on, n=1,200 per arm, over the 200 answerable questions and the 40 relational ones. Only the rule's arm is what would have shipped, and it missed too: relational recall 47.5% (19/40), and p95 30.5 ms, +11.5%. The rule fired on 22 of 40 relational questions, and on 31 of the 200 others. I didn't rerun this or tune anything after seeing it, as the rule says.

How far to trust this:
- **The links were built for the test.** Each A page links to its B plus up to 5 distractors, on 2,080 pages among 200k segments. The gain shows the mechanism works, not that it helps on real wiki content.
- **The recall condition is weak on the existing set.** Those questions' passages are mostly not pages, so expansion rarely adds anything for them. The cost on a vault of mostly pages isn't measured either.
- **Single run on a busy machine.** The 10% limit is on a tail figure, and a quiet machine might read differently. Either way, the result recorded is the miss.
- **The fixture.** 40 questions by a helper who read only the sampled pairs, never the search code, with 110 of 150 pairs rejected, some partly to spread picks across the file. It flagged rel-005, rel-022, rel-025 and rel-040 as doubtful, and these stay in. I wrote the rule without reading the questions, having seen only the helper's summary of them.

**Not run.** `pg_bad_cases` (it would kill the other worker's writers, so the coordinator runs it at landing), `pg_kill_postgres`, `pg_child` and `pg_job_child` (helpers run by their harness), and `pg_full_disk_returns_an_error_and_data_stays_readable` (needs a small disk). The change adds tables to the commit path these cover, and `compact`'s list, but no new failure handling.

**Findings.**
- **Connections aren't bounded.** Each read takes a connection, and `PgStore`'s pool doesn't limit them. My first run of the editor test had 100 threads reading at once, and past the server's `max_connections` of 64 the reads failed with `too many clients`, which reaches a caller as a 503. No update was lost. That run may briefly have refused connections to another worker on the shared server. The test now limits itself to 16 readers, so **100 concurrent editors are shown at the store and facade with limited reads, not 100 unthrottled HTTP clients**. This predates the spec, so I left it.
- **The first Postgres run also found a real bug** that memory tests had missed: embed jobs didn't read pages or blocks, so they never reached the vector index. Fixed, with a test.

**Decisions.**
- **Links point at slugs,** as `slug:<slug>` edges resolved on read, so a rename, a new page or a delete writes nothing to other pages. A new page takes up every link waiting for its slug, and no page can take a slug another holds, even an old one, so a link can't change meaning.
- **Bodies are canonical.** A stored body is exactly its blocks joined, so line endings and blank-line runs are normalized on write and everything else stays.
- **Block identity:** an unchanged block keeps its id even if moved, and an edited one keeps the id of the block in its place.
- **Two reverts.** `RevertPage` adds a revision holding an old one, and is the page feature. A ChangeSet revert undoes the rows, including the revision, and refuses if anything has changed since.
- **One change per page per ChangeSet,** which keeps a version equal to a revision.
- **Duplicate titles** are a 409 when they'd make the same slug, unless a slug is given. Three existing tests shared a title and now don't.
- **Diffs are by block,** not by line, since a line diff on a large page has a quadratic cost and blocks have ids.
- **Search:** a block's document has its nearest heading and not the page title, so a rename re-embeds only the page's own document. Link syntax and claim references aren't indexed. Pages and blocks have no source, so a source or trust filter leaves them out, and their one citation says it has no anchor.
- **Import** creates a page, or with `base_version` edits the page holding its slug. It changes only the body, and refuses a file with a different title rather than dropping it.
- **Upgrade:** the keyword schema moved to 4, so **each existing vault's keyword index rebuilds once**. Old pages get a slug, blocks and a first revision on their first edit, and the vector index only gets them through a rebuild.
- **`expand=on` stays available per request** when k-hop is off by default, with `EXPAND_BY_DEFAULT` false in `search/run.rs`.

**Follow-ups.** Decide whether to rerun k-hop on a quiet machine or on real wiki content, and whether `expand=on` should stay. Bound `PgStore`'s connections (spec 062 or 030). k-hop picks a neighbour's block from the first few per page in one read, so a very long page can crowd others out, and those fall back to their page item. An agent with admin scope can still approve a page delete, as spec 023 noted for spec 025.
