---
id: 040
name: console-shell
description: Build the console shell with login, vault switching, navigation and the generated API client
---

# 040 console-shell

## Goal
A signed-in user can move around the whole console and switch vaults, on real data, with every screen's loading, empty, error and offline states handled the same way.

## Background
**Depends on:** 036, 037. Design: `docs/frontend-architecture.md`.

## Approach
- Generate `packages/api` types from the OpenAPI document with `openapi-typescript`. `FetchApi` with `openapi-fetch`, and `FakeApi` with seeded data for tests.
- `Engine` in `core` with the session and vault use cases. Thin views, with TanStack Query caching what the engine returns.
- Login, logout and session expiry. Vault switcher, with the vault in the URL.
- Layout with sidebar navigation for Ingest, Documents, Search, Pages, Entities, Review, Audit, Chat and Admin, showing only what the token's scope allows.
- A command palette for navigation and objects, global error boundary, toasts for errors with request ids, and an offline banner with reconnect.
- A check that fails when the committed API types differ from the current OpenAPI document.

## Verification
1. Login, vault switch, and logout pass in Playwright against the real backend.
2. A read-scope token sees no admin navigation, and direct links to admin routes show a clear permission message.
3. Stopping the server shows the offline banner within 5 s, and starting it recovers without a reload.
4. A deliberately changed API document fails the type check and the drift check.
5. Pure session and navigation logic has unit tests for happy and bad cases, with no DOM.
6. The initial JavaScript for the login route is ≤200 kB compressed.
7. Reloading any URL returns to the same vault and screen.

## Out of scope
The feature screens and the `ui-coverage.json` check (051).

## Result
A signed-in user can move around the whole console and switch vaults on the real backend, and every screen shows loading, empty, error and offline the same way. Keep. The feature screens themselves are placeholders until their own specs. One choice is left for the user: what to do about the CSP error each modal logs, below.

**Built**
- `packages/api`: `openapi.json`, types generated from it, and pure wire-to-domain translation. `packages/types` has the domain data, including `ApiFailure`.
- `core`: the `Api` and `Preferences` interfaces, and an `Engine` with the session, vault, connectivity and notice use cases. Pure logic is in `session`, `connectivity`, `navigation`, `commands` and `notices`, and `fakeApi` has seeded data with scripted failures.
- `edge-http`: `createFetchApi` on `openapi-fetch`, sending `x-memex-csrf`, which turns every failure into data. `edge-browser`: `browserPreferences`.
- `apps/console`: TanStack Router and Query, the sign-in page, `Guard`, and a lazy `shell/` chunk with the sidebar, vault switcher, palette, toasts and `QueryView`, the one component for the four states. A top-level error boundary and the router's error component show the same page. The offline banner shows on every page but the gallery.
- `tools/api-types.mjs` (`pnpm api:gen`, `pnpm api:check`), and `tools/check-bundle.mjs` (`pnpm budget`). `pnpm typecheck` now runs `api:check --offline` first.
- `pnpm --filter @memex/console e2e:shell`: 17 browser tests on a real debug server, which they start and kill themselves.

**Verification.** Timings are from a busy machine with three other workers building, on a debug binary.
1. Passed. Sign-in, a switch from alpha to beta that keeps the screen, all 9 screens from the sidebar, and sign-out, which removes the cookie and makes `whoami` return 401. No token in `localStorage`. No console errors but the expected `whoami` 401.
2. Passed. A read token sees Documents, Search, Pages, Entities, Review, Audit and Chat. Direct links to `admin`, `admin/tokens` and `ingest` say what scope is needed and what the token has. The palette offers no admin screen. A vault with no grant gives the same message whether or not it exists, and the API refuses too: `DELETE` on the vault returned 403.
3. Passed, in 3 rounds, killing the server with `SIGKILL`. Stop to banner: 2,945, 1,382 and 3,372 ms, against the 5,000 ms limit. Server healthy to banner gone: 782, 178 and 1,305 ms, with a marker on `window` showing no reload. A screen opened while offline says why, and loads by itself on recovery. The design's worst case, for a server that hangs rather than refuses, is 4 s: 2 probes 1.5 s apart, each giving up after 1 s. That case is by calculation, with a unit test on the constants, and wasn't run against a hung server.
4. Passed. With `token_id` renamed in a copy of the document: `pnpm api:check --from <file>` exits 1 and names line 3866. After `pnpm api:gen --from <file>`, `pnpm typecheck` exits 1 with TS2339 in `translate.ts`. With only the document changed and the types stale, `pnpm typecheck` exits 1 at the offline check. Restored, it exits 0.
5. Passed. `pnpm test`: 113 tests in 8 files, in plain Node with no DOM, on `ManualClock`. Bad cases include a 401 while signed out, a server error at startup, cursors that never end, 12 refused `next` values and a badly encoded path.
6. Passed. 109,629 bytes gzip (95,293 brotli, 345,361 raw) against the limit of 200,000. `pnpm budget` reads it from `index.html` and exits 1 at a 50 kB limit. In the browser, the sign-in route loaded that one script, and no shell, overlay or gallery chunk.
7. Passed. Goes to and reloads all 9 screens, an object URL, a URL with a query and hash, and an admin subpath, keeping the path, vault and screen. A signed-out deep link comes back after sign-in, and `/` returns to the last vault.

Also: axe finds no violations on the sign-in page, the shell, a permission message and the palette, the last with the same page-structure rules off as the gallery's test. The shell works by keyboard with visible focus, and has no horizontal overflow at 1024 px. A 500 shows its request id in a toast and on the screen, and Retry works. An expired session returns to the same place after sign-in. `//evil.example` as `next` stays on the origin. A shell chunk that won't load reaches the error boundary.

**Open: the CSP error. The strict policy is unchanged, and this is for the user to decide.** `e2e:shell` fails if the count changes, so a change gets recorded. Measured on the real server:

| Build | CSP errors per modal opening | Radix `Select`, per opening | Wheel behind the palette, shell | Wheel behind the palette, gallery | Outside click closes |
|---|---|---|---|---|---|
| As landed | 1 | 2 | 0 px | 0 px | yes |
| Nonce added to the CSP | 0 | 1 | not run | 0 px | yes |
| Own overlay, nothing else | 0 | 2 | not run | 300 px | no |
| Own overlay with `pointer-events-auto` and a lock through `body.style` | 0 | not run | not run | 0 px | yes |

- **In the palette, the only cost today is the logged error.** The page behind doesn't move: the wheel left a scrolled `main` at 300 px and the window at 0, over 3 openings. The shell scrolls in `main`, not the window, and the palette's overlay isn't inside `main`. Spec 37 saw a wheel scroll the gallery by 300 px. This run measured 0 px there, at one wheel position, probably because `RemoveScroll`'s wheel handler still runs when its `<style>` is blocked, which wasn't checked. That may not cover every position or input, so spec 37's number isn't disproved.
- **A nonce works** without `unsafe-inline`, using `style-src 'self' 'nonce-...'`. `react-style-singleton` reads the nonce through `get-nonce`, including from `window.__webpack_nonce__`. This was only tested by rewriting the header in Playwright. Doing it properly needs a backend change: a new nonce per response and an `index.html` that carries it where an external script can read it. `Select` also needs its `Viewport` given the nonce, for its other error.
- **An own overlay and lock is smaller, and needs no backend change.** Rendering our own overlay element leaves out Radix's `RemoveScroll`, which is all that adds the `<style>`. The last row was a throwaway patch to the palette only, and is reverted. Focus stayed in the dialog, and the page scrolled again after closing. Not checked: pinch zoom, and the layout shift from a disappearing scrollbar, which the shell doesn't have as its window doesn't scroll. `Dialog` would take the same change, and `Select` would still log 2.
- `style-src 'unsafe-inline'` wasn't tested, as the brief says not to add it.
- Recommendation: the own overlay and lock, because it removes the error, keeps the policy and touches only `packages/ui`.
- **New finding:** `Select` logs 2 errors per opening, not 1, and spec 37 hadn't covered it. So the vault switcher is a `Menu`, which logs none. Screens in later specs that use `Select` will add errors.

**Decisions**
- Least scope per screen follows the API: Ingest needs `propose`, since upload does. Admin needs `admin`. Review is open to `read`, since reading changesets is, and spec 046 has to limit its actions. Chat is open to `read`, by spec 049, item 3. A server admin has every scope, as on the server.
- One lost probe doesn't show the banner: two failures in a row do, and one success clears it. Probes run every 1.5 s, so a visible tab adds 0.67 `/health` requests a second, each with one database count. A hidden tab stops.
- A health check answering 503 counts as offline. So the banner's text, "can't reach the server", is also shown when the server is up and its database is down.
- An ordinary call with no answer counts toward offline too, so the banner can come sooner than the next probes.
- If the first `whoami` gets no answer, the session is `unknown` rather than signed out, so an outage doesn't show the sign-in page. The probe that finds the server checks again.
- `next` accepts a plain same-origin path only, in pure code. Switching vault keeps the screen and drops the object id and query, which belong to the old vault.
- A browser logs the 401 from `whoami` on every signed-out load. `e2e:session` now allows that message, for that URL only, which is the one change to spec 37's test. It takes no CSP allowance, and its inline-style allowance is unchanged.
- The palette lists vaults, screens, the first 200 documents, pages and entity types, and filters them in the browser. It can't reach further objects. Entity instances aren't listed, since that API needs a type. Search is spec 043's job.
- Routes are one `/vaults/$` catch-all, and `parseVaultPath` reads it, so links, palette and permission messages share one set of rules. A feature spec adds its own route, which the router ranks above the catch-all.
- `Toasts` and the Radix overlays stay out of the sign-in chunk. `packages/ui` is marked side-effect free to allow that.
- `pnpm.peerDependencyRules` allows `openapi-typescript` 7.13, which asks for TypeScript 5, on our TypeScript 6. It generates correctly, as shown by the type check above.
- `gallery.spec.ts` now waits for the gallery, which became a lazy chunk.

**Follow-ups**
- The server's document repeats the `operationId` `redirect`, on pages and on entities, and the generator refuses that. `tools/api-types.mjs` leaves ids out of the generator's input only, and the committed document keeps them. The real fix is in `crates/`, so it isn't made here.
- `api:check` against the server needs a running one, and nothing runs it automatically. A small test in `crates/memex-http` comparing `openapi()` with `web/packages/api/openapi.json` would catch drift in `cargo test`. It wasn't added, as it changes `crates/`. Until then, a spec that changes the API runs `pnpm api:gen`.
- Not run: a hung server (item 3, above), and `pg_*` tests, as no backend code changed.
