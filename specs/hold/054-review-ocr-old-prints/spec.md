---
id: 054
name: review-ocr-old-prints
description: Decide whether accented-letter errors on old printed documents are acceptable or need a heavier OCR model
---

# 054 review-ocr-old-prints

> **On hold (2026-11-05):** Assumption in force, waiting for the user's review: spec 003's OCR model is acceptable for old prints

## Goal
Have the user decide whether the OCR choice from spec 003 is good enough for old prints, where accented letters fail far more often.

## Background
**Depends on:** 003. Accented-letter error rate is 0.03-0.45% on clean synthetic pages. On real scans it's 17.6% for German, 14.9% for Swedish and 4.6% for Spanish, mostly on old prints. Overall CER still passes.

**Assumption in force:** spec 003's choice stands. Modern documents are the main target, and low-confidence OCR pages get flagged by spec 016. Search folds diacritics, so `å` read as `a` still matches.

## Approach
Ask how much old material the user expects. If it matters, test the PP-OCR server recognizer or a vision model on old prints.

## Verification
1. The user's answer is recorded here.
2. If a new model is tested, the same corpus and scorer as spec 003 are used.

## Out of scope
Fraktur, handwriting and vision LLM OCR, unless the user asks for them.

## Result
Not started.
