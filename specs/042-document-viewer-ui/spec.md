---
id: 042
name: document-viewer-ui
description: Build source browsing and the viewer that shows highlights and plays audio ranges
---

# 042 document-viewer-ui

## Goal
Any claim can be followed to the exact highlighted region of its page or moment of its recording, and highlights are correct on rotated and cropped pages by a pixel test.

## Background
**Depends on:** 017, 028, 040. Design: `docs/frontend-architecture.md`, section Decisions, and `docs/architecture.md`, section Anchors.

## Approach
- Source list with filters, detail, versions, trust-tier change, renditions, segments, reprocess, and flagged pages.
- Page viewer with server images at the allowed DPI values, thumbnails, zoom and page navigation. Pure overlay maths in `core` turns anchor rects and page size into positions.
- A segment and word layer for selecting and copying text, built from word tables, since page images have no text.
- Audio and video player using range requests, with a synced transcript, speakers where known, and click-to-seek.
- A lineage view from any claim to Evidence, anchor, Segment, Rendition and Source, showing model and prompt for derived items. Broken anchors are clearly marked.
- Deep links that open a given source, page and highlight, or a given time range.

## Verification
1. Pixel test: on rotated, cropped and multi-column fixtures, overlays land within 2 px at 100% zoom of the regions labelled in the spec 017 fixtures.
2. Overlay maths has unit tests for all rotations, zoom levels and quads, with no DOM.
3. A deep link to an audio range starts playing within 1 s of the requested time, on a fixture recording.
4. A marker in document text with HTML tags or script appears as plain text, in segments, quotes and transcripts.
5. A 400-page document opens with first page visible within 1 s on a warm cache, and only visible pages are fetched.
6. Bad cases: a broken anchor, a missing blob, a superseded rendition, an erased source and a media type the browser can't play each show a clear state.

**Rules set before measuring.** All run on the real server with its real CSP, in Playwright's Chromium, at device scale 1.
- **Item 1.** 100% zoom means one CSS pixel per PDF point, which is the 72 dpi image at its own size. For each of the 50 labels in `crates/memex-pdf/tests/fixtures/labels.json`, a claim quoting the label's text is committed on the real server, and its deep link is opened. The overlay is found from pixels: a screenshot with the overlay hidden is compared with one showing it. The box around the changed pixels, against the page image's own position, must be within 2 px of the label on all four sides. One side of one label over 2 px is a miss. The same is also recorded at 125% and 200%, with no pass rule there, because a label's error in points grows with zoom.
- **Item 3.** Two numbers on a real transcribed recording, through a link opened in the page and through a fresh load: how far the playing position is from the requested time when playback starts, and the time from opening the link to the position advancing. Pass is both under 1 s, in the median of 5 runs, with each run's number recorded. A browser's autoplay block on a fresh load is reported as what it is, and its time isn't counted as playing.
- **Item 5.** A generated 400-page PDF in this spec's own database. "Warm" means the server has already rendered that page at that resolution and the browser has no copy, so it's the harder case of the two, with a fresh browser context per run. "Open" is from the start of navigating to the document's URL to the first page's image being loaded. Pass is a median of 5 runs at or under 1,000 ms, with all 5 and the load average recorded. Only visible pages are fetched: on opening, the requests for page images are exactly the pages that overlap the screen, and thumbnails only for those in the rail's view, and no word table. After scrolling to page 200, no page image more than the screen's worth away from page 200 has been requested, apart from those seen on the way.

## Out of scope
Annotations, in-browser PDF parsing, and editing documents.

## Result
Built the Documents screen and a viewer other screens can reuse. All six items pass on the real server with its real CSP, in Playwright's Chromium, the only browser set up. Each item says below which of its states had to be injected, because the real server can't make them yet.

**Measured.** Timings are from the final run on a release build, at a load average of about 7 to 12 unless stated. A debug build misses item 5.

1. **Passed: 50 of 50 labels within 2 px at 100% zoom, worst 1.93 px,** by pixels. The overlay is found by comparing a screenshot with it and one without it, and compared on all four sides with `labels.json`. Worst by file: `rotate270_crop` 1.93, `three_column_de` 0.81, `rotate90` 0.79, `two_column_en` 0.61, `rotate180_crop` 0.52, `one_column_sv` 0.52, `cropbox_offset` 0.44, `table_es` 0.43. The load moved from 8 to about 60 during the run, which doesn't affect pixels. **Three limits:**
   - **45 labels go through their own committed claim. The other 5 are a quote that repeats in its segment,** where a claim anchors to the first place. A claim can't reach the labelled place, so their rects come from the same word table at that place and are given to the screen in the lineage answer. The overlay code is the same either way, but those 5 aren't an end-to-end anchor.
   - **The pass depends on 100% meaning one CSS pixel per point,** the rule set before measuring. `rotate270_crop`'s 7 labels are about 1.9 pt off on one side at every zoom, the pdfium and poppler difference spec 015 recorded. They'd miss 2 px at any scale over about 104%.
   - **At 125%, 43 of 50 are within 2 px,** and the 7 misses are those labels, at 2.66 px. **At 200%, 42 of 50:** the same 7 at 3.86 px, and one `three_column_de` label at 61.83 px, whose right edge is past the test's 1400 px window, so it's cut off rather than misplaced. Its other sides are within 0.55 px. These zooms have no pass rule.
2. **Passed:** `overlay.test.ts` has 74 tests, in plain Node with no DOM, covering all four rotations at all 7 zoom levels, quads at every zoom, clamping to the page, rects with no area and a page with no size. `pnpm test` runs 344 in all.
3. **Passed, on a real recording transcribed through `memexvault serve`:** position error 0 ms in all 10 runs, and the time to the position advancing has a median of **68 ms from a click** (65 to 315) and **693 ms from a fresh load** (421 to 741). The position is what the media element reports, so nothing here listened to the audio. Spec 028's item 5 is what checked that ranges hold the right bytes. On earlier runs at higher load, the fresh load's median was 710 ms at load about 340 on release, and 723 to 1,269 ms at 120 to 210 on debug, where **the worst of them missed the rule.** That's the number to expect on a busy machine with a debug build. Seeking used `bytes=1048576-`, answered 206. Playing a range ending at 137,960 ms stopped at 138,120 ms, so 160 ms late.
4. **Passed:** markup appears as text in segments, quotes, claims, a source's name and the word layer, on real uploads, with nothing injected and no CSP report. **Transcript text is replaced at the network,** because a real transcriber won't write markup on demand, so React and the CSP are real and the text isn't.
5. **Passed on a release build, missed on a debug build.** The rule, set before measuring: warm on the server, a new browser with no copy, and a median of 5 at or under 1,000 ms. **Release: median 822 ms, worst 827 ms** at load about 8, and 858 and 905 ms at load about 190. **Debug: median 2,308 ms, worst 2,443 ms** at load about 48, because a warm page image alone takes 788 ms against 72 ms on release. Each open requested exactly pages 1 and 2, the two on screen, plus 7 thumbnails, no word table, and 2 page cells in the DOM. Opening from the list in a running page takes 246 ms the first time and about 60 ms after. A link to page 200 requested pages 200 and 201 only. Scrolling 12,000 px in one go requested only the 2 pages it stopped on. **The document needed the workaround under Backend findings to exist at all.**
6. **Passed:** each state is on the real server and screen, with these differences:
   - **Superseded:** really reprocessed through the screen. Re-anchoring correctly moved the claim to the new rendition, so its row was put back on the old one with SQL, in this database.
   - **Broken:** set on the row with SQL, as only re-anchoring or an audit sets it. The server still returned 1 rect with `broken: true`, and the screen drew none.
   - **Missing blob:** the file was really deleted. See Backend findings for what the server says.
   - **Erased:** spec 030 isn't built, so the row was deleted with SQL. The server gives 404 and an empty evidence list, and the screen says the document may have been erased. **Re-check this when erasure lands.**
   - **A type the browser can't play:** Playwright's Chromium plays AAC and PCM in Matroska, so a generated half-second of ALAC goes through the real stream, and the browser itself refuses it. The path where `canPlayType` says no in advance is scripted.

**CSP, measured early against the real server.** Page images, thumbnails and range-requested media load on the session cookie alone with no change to the policy. The same image address gets 401 with no cookie. Positions go through `element.style`, which the policy allows. No suite run reported a CSP refusal from these screens.

**Spec 028's end-to-end run has now happened.** A 5-minute MP3 went through `memexvault serve` and installed models, linked read-only from the main checkout: `whisper`, `multilingual-e5-small`, `mmarco-mMiniLMv2-L12-H384-v1` and the 3 OCR files at the top of the folder. It took 18 s at load about 7 on release, 28 s at about 370 on release, and 38 to 61 s on debug. It gave 6 segments, and a real claim got a time range and no broken anchor. Item 3's number therefore comes from a real recording, not a word table.

**Trust tier, on the real server.** The screen says that only sources a person marked trusted or verified can auto-commit.
- A person raising unverified to trusted: applied at once, with admin or propose scope.
- A person raising untrusted to trusted: held for review, and the tier is unchanged.
- An agent raising unverified to trusted: held for review.
- A read token: the panel explains that it needs propose scope and offers no way to send it, and Reprocess isn't shown. Sent directly anyway, both routes give 403.
- Someone else changed the source first: the screen says so and nothing is done. The exception is a change held for review, which the server accepts on a stale base. Approving it is where staleness would be caught, and this spec didn't test that.

**Modals: none added, so 0 new errors.** Confirmations are in the page. `e2e:shell` passes, with the palette's existing 3 errors in 3 openings.

**Bundle.** The viewer's chunk `documents-screen` is 51.6 kB, 15.9 kB gzip, loaded lazily. It also loads the shared `data-table` chunk at 12.6 kB gzip and `wiki-common` at 2.9 kB gzip. `pnpm budget`: the sign-in route's initial JavaScript is 120,071 bytes gzip against the 200,000 limit.

**Backend findings, not fixed, as this spec edits `web/`:**
- **A long text PDF can't be ingested on the real server. This one matters.** A rendition job holds a 60 s lease that nothing renews, while transcription renews its own. Hidden-text sampling OCRs every tenth text page, so the 400-page PDF needs 72 s even on release at low load. Its lease expires, it's taken back from its own worker, and it goes dead. Reproduced on release with no workaround: **5 attempts of about 60 s each, all "the worker stopped without finishing", state `dead`.** Any PDF whose rendition takes over 60 s should hit this. The check keeps the lease alive by updating `lease_until_ms` in its own database, so item 5's word table is real, but **this is why item 5's document exists at all.**
- **A missing blob answers 503 "a backend is unavailable, so try again",** which reads as passing when the loss is permanent. The screen shows this with its request id and a Retry button, and adds that the original file may be missing. It's clear enough, but the server's answer should say so.
- Lineage returns rects along with `broken: true`, so a client has to check `broken`, as this one does.

**Decisions.**
- 100% zoom is one CSS pixel per point, and the image's resolution is the smallest allowed that's sharp at that zoom and screen.
- No overscan: exactly the pages overlapping the screen are fetched, and a page waits for 120 ms without scrolling, as rendering costs the server work. Fast scrolling shows placeholders briefly.
- The word table comes whole from `/renditions/{id}`, so it's fetched only when someone presses **Select text**.
- Filters run in the browser, as `/sources` has none, over at most 10,000 sources, and the list says when it stopped short.
- The list's Flagged filter reads the flagged-pages route only when it's on.
- `src/viewer/` knows nothing of sources or URLs. `overlay.ts` and `page-stack.ts` in `core` know nothing of any screen.
- A deep link names a highlight with `?claim=&evidence=`, a page with `?page=` from 1, and a time range with `?t=&end=` in milliseconds. A claim's lineage is at `documents/-/claims/<id>`.
- Autoplay: Playwright's Chromium never blocked it, whatever flags it was given, so `blocked` is 0 in every real run. The state where a browser wants a click is tested with a scripted refusal, which came back ready at 134.7 s, paused, and played on the click.

**Definition of done,** with `crates/` unchanged, so the cargo checks don't apply: `pnpm typecheck`, `lint`, `test` (344), `run licenses`, `run contrast`, `e2e` (6), `budget`, `e2e:documents` (11), `e2e:wiki` (17) and `e2e:shell` (17) all pass. No migrations. No new dependencies.

**Follow-ups:**
- Fix the rendition job's lease, then remove the keep-alive from the check.
- Give a missing blob its own answer from the server.
- Re-check the erased state with spec 030's real erasure.
- Consider a per-page words route, so text selection doesn't load a whole word table. It wasn't sized here.
- A real browser's autoplay block hasn't been seen, so try the fresh-load link in a normal Chrome.
