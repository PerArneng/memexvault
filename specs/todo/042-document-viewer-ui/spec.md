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

## Out of scope
Annotations, in-browser PDF parsing, and editing documents.

## Result
Not started.
