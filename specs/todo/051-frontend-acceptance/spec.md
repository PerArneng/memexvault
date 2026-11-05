---
id: 051
name: frontend-acceptance
description: Prove the whole console works end to end and that all functionality has a UI
---

# 051 frontend-acceptance

## Goal
Prove the console meets its requirements as a whole. One scripted journey runs from upload to chat through the browser, and every API operation has a UI or a recorded reason why not.

## Background
**Depends on:** all of 034-050, and 031. Design: `docs/frontend-architecture.md`, and `docs/architecture.md`, section API conventions.

## Approach
- `web/ui-coverage.json` maps every OpenAPI operation to a screen, or marks it no-UI with a reason. A test compares it with the generated document.
- A Playwright journey against the real backend and Postgres, with a scripted LLM: sign in, create a vault, upload documents and a recording, watch them process, review, create a page and a type, search, follow citations, run an audit, chat, approve a proposal and erase a source.
- axe on every route, a keyboard pass of the journey, and bundle budgets per route.
- Measure the targets from the UI specs in one run on the reference machine.

## Verification
1. An unmapped operation fails the build, shown with a temporary route. No operation is unmapped at the end.
2. The journey passes and can be re-run with one command, with no paid LLM calls.
3. axe finds 0 violations on every route, in the states the journey reaches.
4. A results table: each measured target from specs 036, 040, 041, 043 and 045, with the value and pass or fail.
5. No route's initial JavaScript exceeds its recorded budget, and the journey makes no request to any origin other than the server's.
6. Any failure becomes a follow-up spec, or a target revision with a recorded reason and the user's OK.
7. Screenshots of each main screen are recorded in the Result for the user to review.

## Out of scope
New features, cross-browser testing beyond Chromium and one other engine, and third-party security audits.

## Result
Not started.
