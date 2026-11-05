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
Not started.
