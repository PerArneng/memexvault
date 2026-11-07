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
- **Item 1.** 100% zoom means one CSS pixel per PDF point, which is the 72 dpi image at its own size. For each of the 50 labels in `crates/memex-pdf/tests/fixtures/labels.json`, a claim quoting the label's text is committed on the real server, and its deep link is opened. The overlay is found from pixels: a screenshot with the overlay hidden is compared with one showing it. The box around the changed pixels, against the page image's own position, must be within 2 px of the label on all four sides. One side of one label over 2 px is a miss. The same is also recorded at 133% and 200%, with no pass rule there, because a label's error in points grows with zoom.
- **Item 3.** Two numbers on a real transcribed recording, through a link opened in the page and through a fresh load: how far the playing position is from the requested time when playback starts, and the time from opening the link to the position advancing. Pass is both under 1 s, in the median of 5 runs, with each run's number recorded. A browser's autoplay block on a fresh load is reported as what it is, and its time isn't counted as playing.
- **Item 5.** A generated 400-page PDF in this spec's own database. "Warm" means the server has already rendered that page at that resolution and the browser has no copy, so it's the harder case of the two, with a fresh browser context per run. "Open" is from the start of navigating to the document's URL to the first page's image being loaded. Pass is a median of 5 runs at or under 1,000 ms, with all 5 and the load average recorded. Only visible pages are fetched: on opening, the requests for page images are exactly the pages that overlap the screen, and thumbnails only for those in the rail's view, and no word table. After scrolling to page 200, no page image more than the screen's worth away from page 200 has been requested, apart from those seen on the way.

## Out of scope
Annotations, in-browser PDF parsing, and editing documents.

## Result
Not started.
