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

**Pass rules, set before anything was measured:**

- **Item 1.** A production build served by the real binary, in Playwright's Chromium at 1400 by 1200. The search response is a fixture of 50 items, 10 claims, 30 segments and 10 sections, averaging over 1,000 characters each with query words to mark, fulfilled at the network. Each sample is a new query in a page that's already open: the time from the search request's `responseEnd` to the first frame after the results are in the DOM, taken as `requestAnimationFrame` and then a message task. 5 warm-up samples are dropped, then 50 count. It passes if the 95th percentile is at most 50 ms. The load average is recorded beside it, and a miss stays a miss whatever the load.
- **Item 2.** The first 5 entries of `golden.jsonl` per language whose answer is verbatim in their passage, so 20 in all. Their kinds cycle: a segment in a PDF, a claim in a PDF, a segment in a text file. A passage with characters a PDF's Latin encoding can't hold goes in a text file instead. Each PDF has 3 pages with the passage on page 2 and neighbouring corpus passages around it. The result is found by searching the entry's question in the screen, on a real server with the real models, and its citation is clicked. It passes for an entry if the viewer is on the expected document, and: for a PDF segment, the cited page is showing and every word of the answer lies inside a drawn highlight, within 2 px; for a PDF claim, the highlight's outline matches the answer's words within 2 px on all sides; for a text file, the cited segment is on screen and holds the answer. All 20 must pass.

## Out of scope
Saved searches, alerts, query suggestions and answers generated on the search screen. Chat covers generated answers.

## Result
Not started.
