---
id: 043
name: search-ui
description: Build the search screen with filters, evidence packages and click-through to sources
---

# 043 search-ui

## Goal
A user finds information fast and can see why to trust it, with citations that open the exact region. The UI adds ≤50 ms to the API's response time.

## Background
**Depends on:** 020, 042. Design: `docs/architecture.md`, section Search.

## Approach
- A query box with debounce, and identifier queries treated the same way as in the API. Typed filters and facets, with all search state in the URL.
- Results as the evidence package: claims first, then segments and parent sections, with highlighted quotes, trust tier, epistemic status, conflicts and an explicit "nothing relevant" state.
- Click-through to the viewer, page or entity. An `as_of` control for viewing past states.
- An optional debug panel for per-signal scores and the planner's path, and a rerank toggle.

## Verification
1. Added UI time over the API is ≤50 ms p95 from response received to results painted, measured in Playwright on a fixture of 50 results.
2. Every citation opens the right region, tested on 20 golden-set results.
3. Two conflicting claims show both values with their sources.
4. Queries in English, Swedish, German and Spanish render correctly, including å, ä, ö, ü, ß and ñ, and quotes keep their exact text.
5. Pure logic for filters and URL state has unit tests, including malformed URLs.
6. Bad cases: empty query, no results, unreachable `min_seq`, an expired cursor, and a query with HTML in it.
7. Rapid typing shows only the latest query's results, with earlier requests cancelled.

## Out of scope
Saved searches, alerts, query suggestions and answers generated on the search screen. Chat covers generated answers.

## Result
Not started.
