---
id: 015
name: pdf-rendition
description: Turn PDFs and text files into renditions with word tables, segments and exact anchors
---

# 015 pdf-rendition

## Goal
Each born-digital PDF becomes a Rendition with canonical text, a word table with boxes, and structure-aware Segments.
Anchors round-trip: quote to rects and back to the same text.

## Background
**Depends on:** 014, and the library from 002. Design: `docs/architecture.md`, section Anchors.

## Approach
- `PdfParser` edge in `memex-pdf`, converting all coordinates to the displayed convention and storing page transforms.
- Parsing runs under a timeout and contained failure handling, because PDFs are untrusted input.
- Pure code: canonical text building, heading and paragraph detection, segmenting at 100-400 tokens along structure, and
  the anchor locator.
- Per-segment language detection with a pure-Rust library, storing unknown at low confidence.
- Plain text and Markdown sources use the same model, with offset anchors only.
- Rendition blobs store word tables. A new parser version creates a new Rendition and re-anchors by quote.

## Verification
1. 50 labelled fixture anchors round-trip on rotated, cropped and multi-column pages: rects match within 1 pt.
2. Word F1 and IoU stay within 0.01 of the spec 002 result, through the product code.
3. A re-render with a changed parser version creates a new Rendition. Old anchors re-anchor or get flagged, with none
   silently lost.
4. Bad cases: malformed, encrypted, zero-page and time-bomb PDFs fail one job without harming the server. A quote that
   appears twice resolves using prefix and suffix.
5. Word-table stability: parsing the same file twice gives byte-identical rendition blobs.
6. Language is correct on at least 95% of labelled segments in English, Swedish, German and Spanish, including a mixed-language
   document.

## Out of scope
Scanned pages (spec 016), Office formats, tables as structured cells, and images and figures as objects.

## Result
Not started.
