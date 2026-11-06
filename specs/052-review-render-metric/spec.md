---
id: 052
name: review-render-metric
description: Decide whether blurred pixel correlation is the right render check, after raw correlation failed for every renderer
---

# 052 review-render-metric

## Goal
Have the user confirm or replace the render check from spec 002, so that assumption stops being open.

## Background
**Depends on:** 002. Spec 002 required pixel correlation ≥0.95 with `pdftoppm`. No renderer reached it: pdfium scored 0.858 at 150 DPI and 0.912 at 300 DPI.
I judged this a flaw in the check, since antialiasing and hinting dominate it on thin text.

**Assumption in force:** both images are blurred by 1/100 inch before correlating, and every miss is reviewed by eye.

## Approach
Ask the user whether blurred correlation with visual review of the worst pages is acceptable. If not, choose another check and rerun `tools/compare.py` from spec 002.

## Verification
1. The user's answer is recorded here and in spec 002's Result.
2. If the check changes, the render results are remeasured and recorded.

## Out of scope
Changing the library, unless the new check rejects pdfium.

## Result
Not started.
