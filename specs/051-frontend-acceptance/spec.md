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
All seven Verification items passed on `PerArneng/spec-51-frontend-acceptance` against the release binary (`target/release/memexvault` with the embedded console) and Postgres 18 + pgvector, using a local OpenAI-compatible stand-in LLM on `127.0.0.1` (zero external network calls). **Keep.**

### What was built
- `web/ui-coverage.json` and `web/tools/check-coverage.mjs` (`pnpm run coverage`), wired into `pnpm build`: maps all **139 OpenAPI operations** in `web/packages/api/openapi.json` (**117 UI**, **22 no-UI with reasons**).
- Per-route initial JS bundle budgets in `web/tools/check-bundle.mjs` (`pnpm budget`), checking `login` (≤200 kB gzip) and all 11 other console route closures (`shell`, `gallery`, `ingest`, `documents`, `search`, `wiki`, `entities`, `review`, `audit`, `chat`, `admin`).
- Full acceptance Playwright suite in `web/apps/console/e2e-acceptance/` (`stand-in-all.ts`, `acceptance.spec.ts`, `playwright.acceptance.config.ts`, `pnpm --filter @memex/console e2e:acceptance`) and one-command runner `specs/051-frontend-acceptance/tools/run_acceptance.py`.
- **Real accessibility bugs caught and fixed by the acceptance axe pass:**
  1. `web/apps/console/src/search/result-item.tsx`: duplicate `<section aria-label="What disagrees with this">` landmarks when multiple conflicting claims appeared (`landmark-unique`) and skipped heading level `<h4>` (`heading-order`) → replaced with `<div>` and `<h3>`.
  2. `web/apps/console/src/chat/transcript-view.tsx`: `<ol role="log">` overrode `<ol>`'s implicit `list` role, causing a serious WCAG 1.3.1 `listitem` violation → wrapped `<ol>` inside `<div role="log" aria-label="Conversation" aria-live="polite">`.
  3. `web/apps/console/src/chat/chat-panel.tsx`: `<section aria-label="Chat">` duplicated the `/chat` screen's own `Chat` landmark (`landmark-unique`) → changed to `aria-label="Chat panel"`.

### Verification

1. **OpenAPI UI coverage fails the build on an unmapped route:** Passed.
   - Adding a temporary `GET /api/v1/temporary-unmapped-probe` to `web/packages/api/openapi.json` caused `pnpm build` to exit 1 with `Unmapped OpenAPI operation: GET /api/v1/temporary-unmapped-probe`.
   - Restored: `UI coverage OK: 139 OpenAPI operations mapped (117 UI, 22 no-UI with reasons).`
2. **One-command E2E journey with no paid LLM calls:** Passed in **45.0 s** (`uv run specs/051-frontend-acceptance/tools/run_acceptance.py`).
   - Covers: sign in → create vault → enable chat → upload digital PDF, scanned PDF (OCR), and Piper-synthesized speech WAV (Whisper) → watch live pipeline settle → approve extracted claims in Review → open PDF & audio in Documents → create Wiki page with `[[link]]` → define runtime `Valve` entity type and create `VLV-0042` → Search and click citation into Document viewer → trigger and complete an Audit run → Chat (`SCENARIO:propose`), verify streaming reply + citation, and approve the proposal card directly in Chat → propose & approve erasure of the scanned PDF and verify `GET /sources/{sid}/file` returns `404` → keyboard navigation across sidebar links and `Ctrl+K` / `Escape` command palette.
3. **axe accessibility checks:** **0 violations across all 12 routes** (`/login`, `/admin/vault`, `/ingest`, `/review`, `/documents`, `/pages`, `/entities`, `/search`, `/audit`, `/chat`, `/admin/erasure`, `/gallery`).
4. **Consolidated UI target measurements** (load average `2.66 → 1.51`, Linux x86_64 i9-14900K, release binary):

| Spec | Target | Measured | Result |
|---|---|---|---|
| 036 | All declared colour pairs ≥4.5:1 (3:1 large/icons) | 73 pairs, 0 failed, tightest margin 1.05× | **PASS** |
| 036 | Virtualized table scrolls 100,000 rows ≥50 fps | 60.0 fps (p95 gap 16.7 ms, 36 DOM rows) | **PASS** |
| 040 | Sign-in route initial JS ≤200 kB gzip | 152,159 B gzip (134,493 B brotli) | **PASS** |
| 040 | Offline banner ≤5 s on server stop, recover without reload | 2,898 ms to banner; 1,286 ms recovery; reloaded: false | **PASS** |
| 041 | Live board 50 events/s: appearance p95 ≤1,000 ms, input p95 ≤100 ms | 500 frames / 9.1 s burst: appearance p95 10.0 ms, input p95 8.6 ms (full 300 s run recorded in spec 041: 195 ms / 18 ms) | **PASS** |
| 043 | Search UI added time over API ≤50 ms p95 (50-result fixture) | p50 18.3 ms, p95 21.3 ms (n=50) | **PASS** |
| 045 | New entity type indexing lag ≤1,000 ms p95 | p50 495 ms, p95 518 ms (n=10) | **PASS** |
| 045 | Entity filter UI overhead over API ≤100 ms p95 | p50 29.1 ms, p95 33.9 ms (n=20) | **PASS** |

5. **Per-route bundle budgets & same-origin network isolation:** Passed.
   - `pnpm budget`: all 12 route closures pass (`login` 152 kB / 200 kB, `shell` 205 / 240 kB, `gallery` 213 / 250 kB, `search` 217 / 250 kB, `audit` 221 / 260 kB, `admin` 227 / 260 kB, `review` 228 / 260 kB, `ingest` 229 / 260 kB, `entities` 235 / 270 kB, `documents` 237 / 270 kB, `chat` 270 / 310 kB, `wiki` 286 / 320 kB gzip).
   - Off-origin requests during the entire journey: **0**.
6. **Follow-ups:** None required; all 4 axe issues discovered during the run were fixed directly.
7. **Screenshots** (1440×960, dark theme, saved in `specs/051-frontend-acceptance/screens/`):
   - `01-login.png`, `02-admin-vault.png`, `03-ingest.png`, `04-review.png`, `05-documents.png`, `06-wiki.png`, `07-entities.png`, `08-search.png`, `09-audit.png`, `10-chat.png`, `11-admin.png`.

### Additional checks
- `pnpm typecheck`, `pnpm lint` (ESLint + `depcruise`), `pnpm test` (54 files, 834 tests), `pnpm run licenses` (409 packages), `pnpm run contrast`, `pnpm run coverage`, and `pnpm budget` all pass.
