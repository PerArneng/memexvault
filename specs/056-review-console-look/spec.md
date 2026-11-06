---
id: 056
name: review-console-look
description: "Have the user judge whether the console's dark design system looks professional"
---

# 056 review-console-look

## Goal
Get the user's view on the console's look, since spec 036 leaves that judgement to them.

## Background
**Depends on:** 036. Screenshots: `specs/036-design-system/screens/gallery-1024.png` and `gallery-1920.png`, or run `pnpm build && pnpm --filter @memex/console preview` in `web/` and open `/gallery`.

**Assumption in force:** the look is acceptable, and later screens build on these tokens. Tokens are in one file, so changing colours later costs little.

## Approach
Ask the user to look and say what to change.

## Verification
1. The user's answer is recorded here, and any changes are made to `tokens.css`.

## Out of scope
A light theme.

## Result
(Filled in when the spec moves to done or dropped: what was built, decisions, numbers, follow-ups.)
