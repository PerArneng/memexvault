---
id: 016
name: ocr-rendition
description: Route scanned PDF pages through in-process OCR so they produce the same anchors as digital text
---

# 016 ocr-rendition

## Goal
Scanned and mixed PDFs get the same word tables, segments and box-level references as born-digital ones, and hidden text
in a PDF gets flagged.

## Background
**Depends on:** 015, and the engine from 003. Design: `docs/architecture.md`, sections Anchors, Prompt injection.

## Approach
- A pure per-page routing decision: text layer, image-only, or text layer of doubtful quality.
- `Rasterizer` and `Ocr` traits behind the chosen libraries. OCR boxes converted with `px * 72 / dpi`, plus a quad for
  skewed text. Word confidence stored.
- A low-confidence page is marked, which lowers the confidence on claims from it.
- Hidden-text check: compare the text layer to OCR on sampled pages, and flag words that are invisible or that OCR can't
  see. Also flag invisible Unicode.
- OCR worker pool with a CPU cap, so reads keep their latency.
- This is the first feature needing models, so it adds the `models fetch` command with pinned hashes and a clear startup
  error if models are missing.
- Per-page routes for OCR routing and confidence, and a list of flagged pages (low confidence, hidden text).

## Verification
1. On the spec 003 corpus through the full pipeline: CER and IoU are within 0.5 points of the spike.
2. A mixed PDF routes each page correctly, and anchors on OCR pages highlight the right words visually.
3. A fixture with white-on-white and zero-size text is flagged 100% of the time. Clean fixtures have under 2% false flags.
4. With OCR running, the CPU cap holds and object lookup p95 through the API is ≤1.5× idle. Spec 031 measures search during
   ingest, since search doesn't exist yet at this point.
5. Works offline after `models fetch`, a corrupted model is refused by hash, and missing models give a clear error.
6. Bad cases: a blank page, a huge image page (bounded memory), and an OCR failure on one page that doesn't lose the rest.
7. A fixture with hidden text and one with a low-confidence page both appear in the flagged list.

## Out of scope
Handwriting, vision-LLM OCR, and languages beyond what spec 003 recorded.

## Result
Not started.
