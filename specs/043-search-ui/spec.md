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
Built the Search screen at `/vaults/<id>/search`, from `web/apps/console/src/search/`, with its rules in `packages/core/src/search.ts` and `segment-region.ts` and the wire translation in `packages/api/src/translate-search.ts`. It has a debounced query box, typed filters with facet counts, all state in the URL, the evidence package with marked quotes, trust tier, epistemic status and conflicts, an explicit "nothing relevant", click-through to the viewer, a page or an entity, an `as_of` control, and a debug panel with a rerank toggle. All 7 items pass on a real server with its real CSP, in Playwright's Chromium, the only browser set up, on a debug build. `crates/` is unchanged, so the screen uses the API as it stands.

**Measured.** `pnpm --filter @memex/console e2e:search` has 16 tests. Each number below is from the final full run, with the load average beside it. The machine was busy throughout.

1. **Passed: p95 27.4 ms, p50 16.7 ms, worst 28.3 ms** at load about 380 to 420, by the rule set before measuring. An earlier build scored p95 26.0 ms, p50 17.2 ms at load about 110 to 120, so load moved it little. Of the p50, about 5 ms is from the response to the DOM, 7 ms waiting for the frame and 5 ms style, layout and paint. **Three things to know about this number:**
   - **The first four runs missed, and a change of harness is part of what fixed them.** They gave p95 55.6, 61.7 and 57.2 and then 57.0 ms at load about 130 to 200. A CPU profile showed the screen's own script at under 1 ms per function, and most script running in the page was Playwright's own visibility and accessibility checks, since `expect` polls on the page's main thread. Each sample now runs inside one `page.evaluate`, with nothing polling. The timing definition didn't change, but those four misses are what the screen measures with an automation tool polling the page.
   - **The code changed too.** Text between marks no longer has an element of its own, and a result off screen is laid out only when reached, by `content-visibility: auto`. **So "painted" here means the first screen of results.** All 50 are in the DOM, as the test checks, and the rest are styled as they scroll in.
   - **The response is a fixture of 92,645 bytes fulfilled at the network,** as the item says, so this is the screen's cost alone.
2. **Passed: 20 of 20,** on the real models, with `multilingual-e5-small` and the reranker linked read-only from the main checkout, at load about 180 to 230. All 20 had a vector rank, so that signal was really present. They were 6 segments in PDFs, 6 claims in PDFs, and 8 in text files: 6 from the rule's cycle, and 2 because their passages have characters outside the PDF's encoding. For the claims, the highlight's outline was within 0.02 px of the answer's words on all sides. For the PDF segments, every word of the answer was inside a highlight within 2 px, and the link named page 2, where each passage was placed. The documents took 63 s to be ready to search, and 170 s in an earlier run at load about 490. **A segment's highlight covers the whole segment,** which is what its citation names, not just the answer inside it.
3. **Passed,** with real claims made through the proposals route: each of two claims shows its own value and source, and names the other's value and source, with the source a working link. The server linked them itself, as they share a subject and predicate. A conflict gives source ids only, so a name not elsewhere in the answer takes one more read.
4. **Passed** in English, Swedish, German and Spanish, on text written for the test, with å, ä, ö, ü, ß, ñ, é, ï, curly quotes and `&`. Passage and claim text is compared exactly, as `textContent`, with the API's answer and with the fixture, and quotes with the fixture. The query survives the URL, a reload and going back, and the viewer shows the same text. Terms are marked in any case without changing the text.
5. **Passed:** `search.test.ts` has the URL parsing, 19 malformed values each named and dropped, URLs that aren't well formed, round trips, the term and quote marking, and the citation paths. `segment-region.test.ts` has byte offsets past text of every width, and `search-cases.test.ts` and `translate-search.test.ts` cover the rest. `pnpm test` runs 476.
6. **Passed,** with these differences:
   - **Expired cursor: the API's cursors can't expire,** as they hold only a rank. What a stale link can meet is an unreadable cursor, a 400, and a rank past the end, an empty page. Both are tested on the real server, and each has a way back to the first page. Nothing here exercises an expiring cursor, because none exists.
   - **`not_caught_up` is replaced at the network,** as the real state lasts only a moment. The `min_seq` the log hasn't reached is real.
   - **Empty query:** no request is sent, including for spaces and for pressing Enter twice. **No results:** both the real server's `low_relevance` and a filter that leaves nothing. **HTML** in a query and a document stays text, with no script run, no request made, and no CSP report.
   - **Also:** an unreachable server, and a link with unreadable values, which the screen names rather than dropping quietly.
7. **Passed.** Typing 10 characters 20 ms apart sent 1 request, for the last text. With answers held back at the network, the two earlier requests ended in `net::ERR_ABORTED`, their answers were released afterwards, and a log of every query's results ever on screen held only the last. No alert or offline state appeared, as a cancelled request doesn't count against the connection.

**Also checked on the real server:** rerank on the real model, whose score appears in the debug table, `as_of` by event hiding a later claim and coming back, filters in the URL through a reload, an object's id taking the exact path with the spaces it was typed with, and axe finding no violations, on results with the debug panel. An entity's link is checked with its answer replaced at the network, as making one needs the entity pipeline, and it opens the Entities address, whose screen is spec 045's.

**A bug these tests found:** a query with a hyphen crashed the screen, as `\-` is a syntax error in a pattern with the `u` flag. It's fixed, and `search.test.ts` now runs 15 kinds of punctuation through the marking.

**Decisions.**
- **Segment highlights are computed in the browser.** A citation gives byte offsets, so the viewer's URL takes `segment=`, and `segmentRegion` walks the canonical text with the word table to get one rectangle per line. The table is read only when someone opens such a link. If the text and the table disagree, nothing is highlighted and the screen says why. Text files highlight the segment, and a claim's quote inside it, in the viewer's existing segment list.
- **A page of results is 20 items with a budget of 400 tokens an item,** rather than the API's 4,000, as a person reads more than an agent's default. `limit` is in the URL.
- **While a new search runs, the old results stay, dimmed and saying so,** and are marked stale, so they can't pass for the new ones.
- **The screen explains its own failures,** so its query skips the toast, through a new `meta.ownsErrors` in `query.ts`.
- **No modal, no Radix `Select`, and no new CSP errors.** Filters are toggle buttons and native selects.
- **The chunk is 6.12 kB gzip,** loaded lazily. The sign-in route's initial JavaScript is 120,978 bytes gzip against the 200 kB limit.

**Setup note.** `scripts/fetch-pdfium.sh` got a 504 from GitHub on all 6 tries, so `vendor/pdfium` was copied from the main checkout, read-only.

**Definition of done,** with `crates/` unchanged, so the cargo checks don't apply beyond the debug build the suites ran on: `pnpm typecheck`, `lint`, `test` (476), `run licenses`, `run contrast`, `e2e` (6), `budget`, `e2e:search` (16), `e2e:shell` (17) and `e2e:wiki` (17) passed. **`e2e:documents`: 9 of 11 passed and 1 failed, with 1 skipped.** The failure is item 5, the 400-page open on a debug build, at a median of 2,136 ms against 1,000 ms at load about 115. Spec 042 recorded that miss on debug, 2,308 ms there, and its release build passes. Its requests are the same as spec 042's: pages 1 and 2, 7 thumbnails and no word table. A release build wasn't made here, so this spec doesn't have a passing number for it, and the coordinator's landing run should have one. The skipped test is item 3, as no recording was given, and this spec didn't change the player. Two other tests, trust tier and item 6, first failed with `53300: sorry, too many clients already` from the shared server, which logged 259 such refusals in 40 minutes, and passed when rerun with connections free. One of those retries also got a 503 with 12 connections showing, and its cause wasn't read. No migrations and no new dependencies.

**Landing checks, on the merge with `main` (no code changes there, only follow-up specs), at load about 6 to 60.** `pnpm install`, `pnpm build`, debug and release server builds, `cargo fmt --check`, `cargo deny check licenses`, `pnpm typecheck`, `lint`, `test`, `run licenses`, `run contrast`, `e2e` (6) and `budget` pass. On the release server: `e2e:search` 16 of 16, `e2e:wiki` 17 of 17 and `e2e:shell` 17 of 17. `e2e:documents` on a throwaway Postgres and data folder: 10 of 11 at load about 6 to 12, and the 11th, item 3, failed because the coordinator gave it a `.webm`, while the test uploads its file as `recording.mp3`. With `en_5m.mp3`, item 3 alone passed: position error 0 ms, median to advancing 70 ms from a click and 727 ms from a fresh load, at load about 7. **Spec 042's item 5 on release: median 817 ms** against 1,000 ms (527 to 828 ms) at load about 8, so the debug miss above doesn't apply to release. The user accepted item 1's narrower meaning of "painted", the first screen of results, when they approved landing.

**Follow-ups.** None needed for this spec. An entity's item can be clicked through end to end once spec 045 lands.
