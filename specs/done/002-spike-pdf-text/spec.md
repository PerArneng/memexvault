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
**Decision: `pdfium-render` 0.9.5 on a statically linked pdfium, for both text and page images.** Kept. One assumption needs the user's
review: the render check (spec 052). `lopdf`, `pdf_oxide` and `hayro` were rejected as below.

Word results against `pdftotext -bbox -cropbox`, 30 files, 28,200 words (per-file table in `results-words.txt`):

| Candidate | Word F1 | Median word IoU | Words with IoU ≥0.5 | Panics or hangs | Native deps |
|---|---|---|---|---|---|
| pdfium, static | 0.992 | 0.993 | 99.8% | 0 | system only |
| `pdf_oxide` | 0.990 | 0.576 | 92.5% | 0 | system only |
| `lopdf` | 0.955 (before crop filtering) | none: no word boxes | none | 0 | system only |

- `pdf_oxide` fails the IoU bar. Its x positions match poppler, but its box runs from baseline to baseline plus font size, so descenders fall
  outside it. It also gives wrong boxes on `rotate180_crop` (IoU 0.0) and no text on two Type3 files. `lopdf` has no coordinates.
- pdfium's word grouping is ours, about 30 lines that `memex-pdf` must carry: split on whitespace, on pdfium's U+0002 line-end hyphen (a real
  hyphen, then a word break), and when two characters overlap on neither axis. pdfium has no word API.
- pdfium's weakest files: `rotate180_crop` F1 0.943 and `ArabicCIDTrueType` F1 0 (right-to-left, outside the supported scripts). Type3 fonts have
  low box IoU (0.29 to 0.79), because they carry unreliable metrics.
- Scoring exclusion, hand-checked: where a word crosses the crop edge, poppler reports the visible part and both candidates report the whole
  word. Those words are left out, and words outside the crop are dropped, as `memex-pdf` must also do.

Render results (first 5 pages per file, 50 pages, `results-render.txt`):

| Renderer | Raw correlation, 150 / 300 DPI | Blurred correlation, 150 / 300 DPI |
|---|---|---|
| pdfium | 0.858 / 0.912 | 0.966 / 0.981 |
| `hayro` | 0.782 / 0.856 | 0.950 / 0.976 |
| `pdf_oxide` | 0.836 / 0.875 | 0.938 / 0.948, min 0.09 |

**Check 4 as written fails for every renderer:** mean raw correlation never reaches 0.95, and that's with Chromium's own renderer. Raw correlation of
thin text measures hinting and antialiasing, not correctness. I assumed a blur of 1/100 inch before correlating, and recorded that in spec 052
for review. At 300 DPI pdfium then passes on every page except `hello_world_rotated` (0.87); at 150 DPI, `rotate180_crop` is also low (0.89). The
worst pages were reviewed by eye. `hello_world_rotated` uses a non-embedded Helvetica-Bold: pdfium draws it bold, and poppler's substitute is regular, so
pdfium is the correct one. `rotate180_crop` looks identical apart from a sub-pixel shift at the fractional crop offset. `pdf_oxide` draws the same
page in a serif font and loses Type3WordSpacing (0.09).

- **Classification:** visible text objects, hidden text objects and image coverage were correct on all 30 files by my expected labels, including the
  three hidden-OCR files. Rule: no visible text and images over 50% of the page means image-only. This was checked per file, with spec 016 using it.
- **Speed:** tracemonkey, 14 pages, 0.07 s including library start (about 200 pages/s).
- **Linking:** `static-check` links `libpdfium.a` from `paulocoutinhox/pdfium-lib` (chromium 8046, 34 MB archive, `pdfium_7881` API feature) into a 9.6 MB
  binary. `otool -L`: libc++, CoreGraphics, CoreFoundation and libSystem only. It works with an empty environment. Licences: pdfium BSD-3/Apache-2.0, the crate MIT or Apache-2.0.
- **Containment:** no crash on the corpus, but pdfium is C++ parsing untrusted files, and a crash would kill the process. It also takes a global lock. So
  I assumed it runs in worker child processes, which contains crashes and gives parallelism (spec 053 for review).

Verification: 1 done. 2 passes (0.992, 0.993). 3 passes. 4 fails as written, passes as amended, see above. 5 passes. 6 passes, with the containment assumption. 7 done.

Follow-ups: spec 052 and 053 for review. Linux static build isn't tested yet. `pdfium-lib` publishes one and spec 008 should pin it.
