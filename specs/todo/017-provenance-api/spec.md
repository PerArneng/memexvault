---
id: 017
name: provenance-api
description: Expose lineage from any object to its source region, with highlighted page images and original files
---

# 017 provenance-api

## Goal
From any claim, page block or entity field, a caller can follow the chain to the original document and see the exact
region highlighted.

## Background
**Depends on:** 015. Design: `docs/architecture.md`, sections Anchors, Data model.

## Approach
- A lineage API: object, Evidence, anchor, Segment, Rendition, Source. Includes extractor and Derivation details and a check
  that the quote still resolves.
- A page render API returning a PNG with highlight overlays, and a JSON form with rects and page size so clients can draw
  their own.
- Original file download with HTTP Range support.
- Superseded versions stay resolvable and are marked as such.

## Verification
1. Pixel test: for fixtures including rotated and cropped pages, the highlight overlay covers the target word's rendered
   pixels with IoU ≥0.9.
2. The rect JSON and the PNG agree with each other within 1 pt.
3. An object from an older Rendition still resolves and is marked superseded.
4. Bad cases: a wrong-vault token gets not-found; bad page numbers and DPI above the limit are refused; a broken anchor is
   reported as broken, not guessed.
5. Range requests return correct bytes, verified against the blob hash.

## Out of scope
Audio ranges (spec 028), annotation UI and caching of page images (spec 021).

## Result
Not started.
