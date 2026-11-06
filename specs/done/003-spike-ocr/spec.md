---
id: 003
name: spike-ocr
description: Choose the in-process OCR engine by measuring accuracy, word boxes and speed on scanned pages
---

# 003 spike-ocr

## Goal
Pick the OCR engine for `memex-ocr`. Pass, on 100 scanned pages: character error rate ≤3%, median line IoU ≥0.7 and at
least 20 pages per minute on the reference machine, in-process with local model files.

## Background
**Depends on:** 002 (rasterizer). Design: `docs/architecture.md`, sections Decisions, Anchors.

Candidates from crates.io on 2026-11-05: `ocrs` 0.13 on `rten` 0.27 (pure Rust) and `oar-ocr` 0.10 (ONNX, also does
layout). Tesseract is excluded because it's a system dependency. Language coverage is the main risk: an engine must read English, Swedish,
German and Spanish, including å, ä, ö, ü, ß and ñ. Check whether each candidate has a suitable model.

Spec 005 also picks an ML runtime. Two runtimes in one binary cost size and complexity, so this spike records the
runtime each candidate needs.

## Approach
- Throwaway crate in this spec's folder.
- Corpus: 70 pages rendered from born-digital PDFs and degraded (blur, noise, skew up to 5°, 150 DPI), which gives exact
  ground truth for text and boxes, and 30 real public-domain scans with hand-labelled lines. At least 20 pages per language, across both groups. Sources in `corpus.md`.
- Measure CER per language, error rate on accented letters, WER, line and word IoU, skew handling, speed at 1, 4 and 8 threads, peak RSS, model size and model licence.
- Check whether word-level boxes come out directly or must be derived from line boxes.

## Verification
1. Results table per candidate and degradation type.
2. The chosen engine has CER ≤3% in each of the four languages, line IoU ≥0.7 and ≥20 pages/min overall. Results on real scans are also reported on
   their own, not hidden in the average.
3. Model licences allow redistribution or are recorded as user-installed.
4. Works with the network off and `otool -L` shows system libraries only.
5. Accented-letter error rate is reported per language, and any gap against the four languages is raised with the user.
6. `docs/architecture.md` records the choice, runtime and word-box method.

If nothing passes, put the spec on hold and ask the user, naming the options (statically linked engine or a heavier
vision model).

## Out of scope
Page routing, the pipeline, layout analysis beyond what the engine provides, handwriting, and vision LLM OCR.

## Result
**Decision: `oar-ocr` 0.10.2 with the PP-OCRv6 small detector and the PP-OCRv5 Latin recognizer, on ONNX Runtime through `ort` 2.0.0-rc.13, statically linked.** Kept. Accented letters on old prints are a real gap, raised in spec 054.

`ocrs` 0.13 is rejected without a run: its alphabet has no å, ä, ö, ü, ß or ñ, so it can't read three of the four languages. Its model URL also refused downloads.

All runs at 8 threads, 100 pages (`results.txt` has every table):

| Configuration | Pages/min | Peak RSS | Line IoU | Real CER en/sv/de/es | Real CER, all |
|---|---|---|---|---|---|
| **v6 small detector with v6 settings, Latin recognizer (chosen)** | 34 | 0.79 GB | 0.828 | 0.62 / 1.39 / 3.36 / 2.85% | 1.93% |
| v6 small detector, default settings, Latin recognizer | 38 | 0.63 GB | 0.763 | 0.54 / 1.41 / 3.30 / 10.33% | 3.63% |
| v6 small, its own recognizer | 32 | 1.15 GB | 0.828 | 0.60 / 1.69 / 6.87 / 2.90% | 2.76% |
| v6 medium, its own recognizer | 7 | 1.42 GB | 0.858 | not better | see `results.txt` |
| v5 mobile detector, Latin recognizer | 44 | 0.88 GB | 0.719 | 2.65 / 1.41 / 3.28 / 7.96% | 3.70% |
| v5 server detector, Latin recognizer | 29 | 1.67 GB | 0.745 | 4.13 / 1.46 / 3.30 / about 24% before the column fix | see `results.txt` |

The chosen configuration in detail:
- **CER per language, all pages:** English 0.17%, Swedish 0.23%, German 0.57%, Spanish 0.67%. All meet ≤3%.
- **CER by degradation, synthetic:** clean 0.19%, blur 0.12%, noise 0.14%, skew 3° 0.07%, skew 5° 0.15%. Skew needs no correction step.
- **Real scans only:** German is 3.36%, above 3% on its own. Worst page is `de_02` at 9%. The pass mark applies to all pages together, so this passes, and I'm reporting it separately as the spec asks.
- **Accented letters, error rate:** synthetic 0.03-0.45%. Real: German 17.6%, Swedish 14.9%, Spanish 4.6%. Errors cluster in old prints: one 1780s Swedish law book has 59 of Swedish's 88 errors. Modern-type Swedish pages have 1-2 errors in 57-124 letters. Spec 054 has this for review.
- **Speed:** 11 pages/min at 1 thread, 32 at 4, 34 at 8, against the ≥20 target. So 4 threads per worker is the sensible setting. A page takes about 1.8 s.
- **Model files:** 17 MB for the chosen set, sha256 prefixes `d73e0058b7a8086b` (detector), `e3a6bfeea1c8a01d` (recognizer), `ccbcc45730b3fbbd` (dictionary). PaddleOCR models are Apache-2.0, and we fetch them from the `oar-ocr` v0.3.0 and v0.7.0 release assets.
- **Word boxes:** `return_word_box` returns one box per character, including spaces, from the recognizer's alignment. `memex-ocr` groups them into words at the spaces.

Findings for spec 016:
- **Detector settings matter.** With default settings, lines merged across a column gutter on two-column pages. The v6 settings (score 0.2, box 0.45, unclip 1.4) fixed this.
- **Reading order isn't provided.** The engine returns regions in no useful order, so `memex-ocr` must order columns itself. My scorer tries one and two columns, which is also a limit of this test: pages with three columns or sidebars aren't measured.
- **Dictionary gap:** ¡ isn't in the dictionary, so it can't be read. Quote verification should tolerate that.

Verification: 1 done. 2 passes (combined), real scans reported on their own. 3 passes. 4 passes: all inference ran under `sandbox-exec` with network denied, and `otool -L` lists system libraries only, including CoreML, which isn't used. 5 raised in spec 054. 6 done.

**Runtime for spec 005:** ONNX Runtime through `ort` 2.0.0-rc.13, pinned exactly by `oar-ocr`. Binary is 30 MB. A release candidate is a risk spec 005 also records.

**Corpus change:** real text is Wikisource's proofreading rather than my hand labels, so real pages have no line boxes (`corpus.md`).
