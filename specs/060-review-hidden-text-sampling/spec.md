---
id: 060
name: review-hidden-text-sampling
description: Decide how many pages of a long document get checked against OCR for hidden text
---

# 060 review-hidden-text-sampling

## Goal
Have the user decide how thorough the hidden-text check should be, since each checked page costs an OCR pass.

## Background
**Depends on:** 016. Hidden text is a prompt-injection route. Text a reader can't see can still reach an extraction model. OCR comparison catches white-on-white text and invisible render modes, at about 3 s of CPU per page at the current settings.

**Assumption in force:** check every page that declares invisible text objects, the first 3 text pages and every 10th. So white-on-white text on an unsampled page of a long document isn't flagged. The controls in `docs/architecture.md` still limit what such text can do: extraction has no tools, and quotes are verified.

## Approach
Ask whether to keep the sample, check every page, or check every page for untrusted sources only.

## Verification
1. The user's answer is recorded here, and `hidden_check_pages` matches it.

## Out of scope
Detecting text rendered in near-background colours by colour analysis.

## Result
(Filled in when the spec moves to done or dropped: what was built, decisions, numbers, follow-ups.)
