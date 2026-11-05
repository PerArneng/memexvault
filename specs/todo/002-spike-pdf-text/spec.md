---
id: 002
name: spike-pdf-text
description: Choose the PDF library for text, word coordinates and page rendering by measuring candidates
---

# 002 spike-pdf-text

## Goal
Pick the PDF stack that `memex-pdf` will use. It must extract words with accurate boxes and render pages with no system
library. Pass: on 30 hard PDFs, word F1 ≥0.98 and median word bbox IoU ≥0.8 against ground truth, and pages render
correctly.

## Background
**Depends on:** none. Design: `docs/architecture.md`, sections Decisions, Anchors.

Bounding boxes are the base of all document provenance, so a weak choice here is expensive later. Candidates from
crates.io on 2026-11-05: `pdf_oxide` 0.3.78, `lopdf` 0.47, `pdfium-render` 0.9.5 (needs a statically linked pdfium to
meet the no-system-library rule), and `hayro` 0.9 as a pure-Rust rasterizer. AGPL libraries such as MuPDF are excluded,
because the repo is Apache-2.0.

## Approach
- Throwaway Rust crate in this spec's folder. Nothing goes into a product crate.
- Corpus of 30 public or generated PDFs, fetched by a script into a git-ignored folder, with sources and licences listed
  in `corpus.md`. It must cover: multi-column, rotation, CropBox offset, tables, ligatures, Type3 fonts, an empty-password
  encrypted file, an existing hidden OCR layer, and image-only pages.
- Ground truth from `pdftotext -bbox` as a dev-time tool only. Poppler is never linked. Hand-check the pages where it
  disagrees with every candidate.
- For each candidate measure word F1, bbox IoU after converting to the displayed coordinate convention, reading order,
  pages per second, panics and hangs (with a timeout), and native dependencies.
- Measure whether a page can be classified reliably as text or image-only, since spec 016 routes on this.
- Render pages from every combination worth testing at 150 and 300 DPI.

## Verification
1. A results table per candidate with all measures, per corpus category.
2. The chosen text stack has word F1 ≥0.98 and median bbox IoU ≥0.8.
3. Text-or-image-only classification is correct on all corpus pages, or misses are listed with cause.
4. Render check: pixel correlation with `pdftoppm` output ≥0.95 on the corpus, plus visual review of the 5 worst pages.
5. `otool -L` on the test binary shows system libraries only.
6. No candidate that panics or hangs on the corpus is chosen unless the failure can be contained, with evidence.
7. `docs/architecture.md` records the choice and the `memex-pdf` provisional marker is removed.

If nothing passes, put the spec on hold with the numbers and ask the user. Don't lower the bar silently.

## Out of scope
OCR (spec 003), segmenting, storing anything, and non-PDF formats.

## Result
Not started.
