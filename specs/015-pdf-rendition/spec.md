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
- Routes for rendition list per source and rendition detail (parser version, canonical text, word table). Reprocess, as a job. Segment list per rendition and segment get, with language. A list of re-anchor failures.

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
7. Reprocessing through the route creates a new Rendition, and failed re-anchors appear in the list.

## Out of scope
Scanned pages (spec 016), Office formats, tables as structured cells, and images and figures as objects.

## Result
Built the pipeline from upload to segments. `memex-pdf` parses with statically linked pdfium, in a child process: `memexvault pdf-worker` reads bytes on stdin and writes JSON, and `rendition_job.rs` runs it with a time limit. Pure code in `memex_core::rendition` builds canonical text, segments, the anchor locator and language detection (`whatlang`). `Server::build_rendition` writes the blobs and returns rows, which commit with the job. Routes: `sources/{id}/renditions`, `sources/{id}/reprocess`, `renditions/{id}`, `renditions/{id}/segments`, `segments/{id}` and `reanchor-failures`. `serve` starts two rendition workers. `scripts/fetch-pdfium.sh` fetches the pinned pdfium, checked by hash, into the git-ignored `vendor/`, and `.cargo/config.toml` points the build at it.

**Verification.**
1. 50 anchors labelled by a second tool (poppler) on rotated, cropped and multi-column fixtures round-trip from quote to rects and back to the same text. 43 are within 1 pt on all four sides, the worst of those 0.47 pt. **7 labels on `rotate270_crop` differ by up to 1.87 pt on one side.** That's a non-embedded Helvetica where pdfium's ascent is taller than poppler's, and the edges along the text match, so the box is a little tall rather than misplaced. The test allows 2 pt on that side of that file, and spec 017's pixel test checks whether highlights really cover the word.
2. Product code against spec 002's scorer, on the same 31 PDFs: **word F1 0.9923 (spike 0.9923), median IoU 0.994 (spike 0.993).**
3. A changed parser version makes a new Rendition and supersedes the old. A quote that survives moves to the new segment with a fresh anchor. A quote that's gone stays with `anchor_broken` set and appears in `reanchor-failures`. None is lost.
4. Bad cases, through jobs with the real worker: a password-protected PDF, zero pages, a fake PDF and a truncated one each fail one job with a clear reason, and the next job still runs. A hang is killed at its limit, within the time limit plus 10 ms polling, and a crash or nonsense output fails cleanly. A quote appearing twice resolves by prefix and suffix.
5. Parsing twice gives identical rendition blobs: tested on 3 fixtures directly, and through reprocessing, where the blob hashes match.
6. Language on 1,736 labelled Wikipedia leads in the four languages: 99.6% right, 0.1% wrong, the rest unknown. Cut to 200 characters: 97.2% right, 0.7% wrong. Target ≥95%. A mixed document gets each segment's language in order. The corpus isn't committed, so that test is `#[ignore]`d and takes a path.
7. Reprocessing through the route creates a new Rendition, tested over HTTP and with the real server below, and failed re-anchors appear in the list.

**End to end,** with the release binary on Postgres: upload a PDF over HTTP, the job finishes, the rendition has 2 pages and 6 segments in German with page numbers, and a password-protected upload ends in the dead-letter list with its reason. Binary 18.4 MB, linking system libraries and frameworks only (`libc++`, CoreGraphics, CoreFoundation, SystemConfiguration, `libiconv`, `libSystem`).

**Bugs the fixtures found and tests now cover.** A quote ending in a non-ASCII letter panicked in offset mapping. IDs created in the same millisecond sorted randomly, which put segments out of document order, so ids now have a counter after the timestamp.

**Decisions.**
- Canonical text puts a space within a line, a newline between lines, and a blank line for a wide gap, a jump back up the page (a new column) or a new page. Rotated pages get each word on its own line, since pdfium reports them stacked, and a quote there gets one rect per word. Whitespace differences don't affect quote matching.
- The worker is the same binary with a subcommand. It has a time limit but no memory limit or OS sandbox yet, as spec 053 records.
- A file over 512 MiB fails its job with that reason, since the parser takes bytes.

**Not covered.** Image-only pages produce no words here, and spec 016 adds OCR. No heading detection for bold text of the same size. `build_rendition` reads all old segments for re-anchoring, which is fine at document scale.
