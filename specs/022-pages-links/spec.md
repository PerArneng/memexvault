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
Not started.
