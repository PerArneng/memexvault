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
Built OCR into the rendition pipeline. Pure code in `memex_core::ocr`: per-page routing, resolution choice, column ordering, grouping character boxes into words, `px * 72 / dpi` conversion with a quad, and hidden-text checks. `memex-ocr` is the engine edge with spec 003's models. `memexvault ocr-worker` reads one page in a child process. `models.rs` has pinned URLs and hashes, `memexvault models fetch` and `models check`, and `serve` refuses to start without the files. Page results are on the Rendition, shown by `renditions/{id}`, with `flagged-pages` listing what needs a look.

**Verification.** OCR tests are ignored unless `MEMEX_TEST_MODELS` names a models folder.
1. Spec 003's 100-page corpus through the product's engine and scored by spec 003's scorer: **identical to the spike.** Combined CER en 0.17%, sv 0.23%, de 0.57%, es 0.67%, real pages 1.93%, line IoU 0.828. 26 pages/min at 4 threads. This goes through the engine crate, which takes images, not through the whole upload path, since those are PNGs.
2. A mixed PDF routes page 0 to the text layer and page 1 to OCR, with mean confidence 0.992. An anchor on the OCR page resolves. I drew the word boxes over the rendered page and looked: each box sits on its word, in both columns and in reading order. Spec 017's pixel test measures this.
3. Hidden text, all flagged: white on white (30 words), tiny white text between lines (120), invisible render mode (30) and 0.4 pt text (15). Clean pages: **31 checked, 0 false flags**, in four languages and four font sizes.
4. Reads during OCR, on the real server against Postgres, with 40 documents queued and 4 clients: `GET source` p95 7.9 ms idle and **7.0 ms with OCR running, 0.88× idle**. Target ≤1.5×. An earlier setting, two workers at 4 threads each, gave 1.88×, so workers now use 2 threads each, and run at `nice 10`. That's about 21 pages/min across the two. `load.py` and `load-results.txt` are in this folder.
5. `models fetch` downloaded the 17 MB of files from the pinned URLs and they verified. `serve` with no models names the file and the command. A byte changed in a file of the right size is refused by hash, by `models check` and by each worker, and `fetch` replaces it. After fetching, OCR needs no network: spec 005 checked the runtime that way, and nothing here reaches out.
6. Bad cases: a blank page goes nowhere. A page 200 inches square renders at 30 dpi rather than the 11 GB 300 dpi would need, and `dpi_for` bounds the longest side at 6,000 px. A failure on one page records `ocr_failed` on it, and the job finishes with the rest.
7. Hidden-text fixtures all appear in the flagged list, and the low-confidence mark is unit-tested. No fixture really produced a low-confidence page, since the real scan set is PNGs here.

**Decisions.**
- A page declaring invisible text objects, the first 3 text pages, and every 10th get compared against OCR. This costs one OCR per sampled page, so white-on-white text on an unsampled page of a long document goes unnoticed. Spec 060 asks whether that's acceptable. Zero-size text and invisible Unicode are checked on every page.
- A page is flagged at 3 or more unseen words and at least 5% of its words, since OCR misses a word here and there.
- A flagged OCR-layer page stays flagged: `hidden_render_mode` looks just like an OCR layer that doesn't match the image, and a page where the layer matches isn't flagged.
- One child per page isolates failures and costs about 0.3 s of model loading each time. A per-document child could save that later.
- Workers run `nice 10`, which helps against host processes but not against Postgres inside Docker's VM, so the thread limit is what made the difference.

**Corrections made while closing this.** The final full test run found 3 failures that an earlier run hadn't reached, because it stopped at a compile error. One was a real bug: `edit_distance` used the wrong diagonal, which made hidden-word comparison stricter than intended. Another was a 36 dpi floor that broke the memory bound. After the fixes, all tests and every OCR measure above were rerun, with the same flags and no false ones. `deny.toml` now allows NCSA, a permissive licence reached through the `image` crate's AV1 encoder. The binary is 48.4 MB, and `otool -L` lists system libraries and frameworks only, now including CoreML and Foundation through ONNX Runtime's build, though CoreML isn't used.

**For spec 024:** claims from a page marked `low_confidence` should have their confidence lowered, as this spec's design says. No claim-making code exists yet.
