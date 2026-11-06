---
id: 037
name: browser-session
description: Let browsers sign in with a session cookie and serve the embedded console safely
---

# 037 browser-session

## Goal
A browser can sign in and use every API, image, audio and stream route, and the console is served by the `memexvault` binary with no separate web server.

## Background
**Depends on:** 012. Design: `docs/architecture.md`, section API conventions, and `docs/frontend-architecture.md`, section Security.

Tags such as `<img>` and `<audio>` and the browser's `EventSource` can't send bearer headers, and tokens shouldn't be readable by page scripts.

## Approach
- `POST /api/v1/session` exchanges a token for a random session id in an HttpOnly, Secure when on TLS, SameSite=Strict cookie. Sessions are stored
  hashed, expire through `Clock`, and end when the token is revoked. Logout route, and a whoami that works for both kinds of auth.
- CSRF defence: SameSite plus a required custom header on all state-changing cookie requests.
- Embed the built console with `rust-embed` in `memex-http`. Fingerprinted assets get long cache lifetimes, and `index.html` gets none. Unknown non-API paths
  return `index.html`. A build without built assets returns a clear error page.
- Security headers on all responses, including `Content-Security-Policy` with `default-src 'self'`, `frame-ancestors 'none'` and `nosniff`.
- Original downloads use `Content-Disposition: attachment`, so uploaded HTML or SVG can't run on our origin.
- Vite's development proxy, documented in `AGENTS.md`. No CORS is needed in production.

## Verification
1. The spec 012 scope matrix passes again through cookies, with the same results as bearer tokens.
2. A cross-site POST without the custom header is refused, and a revoked token ends its sessions at once.
3. The cookie isn't readable from page script, shown in Playwright.
4. An uploaded HTML file with a script, opened through the download route, doesn't run in the console's origin, shown in Playwright.
5. Bad cases: expired session, tampered cookie, a path-traversal request for assets, and a login with a wrong token, which is rate-limited.
6. The release binary serves the console with the network off, and `otool -L` lists system libraries only.

## Out of scope
SSO, OAuth, passwords and TLS termination.

## Result
A browser can sign in and use every API route with a session cookie, and the binary serves the console by itself. Keep.

**Built**
- `POST /api/v1/session` and `DELETE /api/v1/session`. Sessions are stored as SHA-256 hashes in `memex_server.sessions` (server migration 4). They last 12 hours by `Clock`, and each request runs the token's own checks, so revoking the token or disabling the principal ends them at once. `whoami` answers for both kinds of auth, because every route authenticates in one place.
- A CSRF guard: a state-changing request that a cookie would authorize, or a sign-in, needs `x-memex-csrf`. A bearer header decides alone when present, and a bad one never falls back to a cookie.
- 10 failed sign-ins a minute per peer address (`LoginLimiter`, pure, in `memex-core`), then 429 with `Retry-After`, even for the right token. It holds at most 10,000 clients, and past that new ones share a count.
- `crates/memex-http/src/browser.rs`: security headers on every response, and the embedded console. `assets/` files get `max-age=31536000, immutable`, `index.html` gets `no-store`, and other files get `no-cache`. Unknown paths outside `/api` get `index.html`, and `/api` paths keep the JSON 404. A build with no console returns a 503 page saying how to add it.
- The download route adds a `sandbox` CSP to its existing `attachment` and `nosniff`.
- `build.rs` makes Cargo rebuild when `web/apps/console/dist` changes, which was seen working. `MEMEX_API_URL` points Vite's proxy at another server.
- `pnpm --filter @memex/console e2e:session`, separate from `pnpm e2e` as it needs a real server.

**Verification.** Timings are from a busy machine, with two other workers running.
1. Passed. `the_scope_matrix_has_no_gaps_and_denials_change_nothing` runs all 16 cases for 4 roles through bearer tokens and then cookies, and status, error code and message are equal. `every_route_in_the_contract_takes_a_cookie_and_refuses_no_credentials` walks the OpenAPI document, so a new route is covered by itself. An `<img>` is shown sending the cookie in Playwright. No stream or audio routes exist yet: they use the same `authed`.
2. Passed, in memory and in Playwright. With the cookie and no header: 403 `csrf`, and no state change. From another site (`localhost` against `127.0.0.1`), a `no-cors` POST created nothing, and a fetch with the header was blocked by the browser. After revoking, `whoami` went from 200 to 401 on the next request.
3. Passed in Playwright: `httpOnly: true`, `sameSite: Strict`, and `document.cookie` is empty at `/` and also at `/api/v1/whoami`, because the cookie's `Path` alone would hide it from `/`.
4. Passed in Playwright, with a control: the same HTML served as a plain page does set `localStorage`. Through the download route, opened directly, by a link click, in a frame and as a script, it downloads as `evil.html` and sets nothing.
5. Passed, in memory and on the live server. Expired: 401, on the clock, and by setting `expires_at_ms` in Postgres. Tampered: 401. Traversal, 9 literal and encoded forms: `index.html`, never the file. Wrong token: 401 ten times, then 429 with `Retry-After: 60`.
6. Passed. The network was off for the server only, with no system change: `sandbox-exec` with a profile that denies all network but loopback (below). Under it, `curl` to `example.com` fails with `Could not resolve host`, and to `1.1.1.1` with `Couldn't connect`. The Playwright browser ran outside the sandbox and made no request off the origin, and that test passed on the release binary. `otool -L`: 9 libraries, all under `/usr/lib` or `/System/Library`.

```
(version 1)
(allow default)
(deny network*)
(allow network-bind (local ip "localhost:*"))
(allow network-inbound (local ip "localhost:*"))
(allow network-outbound (remote ip "localhost:*"))
```

`whoami` over one connection, 1,000 requests each, second round: bearer p50 1.25 ms, p95 2.28 ms. Cookie p50 1.47 ms, p95 2.78 ms, from one more lookup. The console adds 744 KB to the binary's 61 MB.

Postgres tests on `memex_s037`: 10 of 10 selected passed. Not run on the final code: `pg_kill_postgres` (kills the shared container), `pg_child` and `pg_job_child` (helpers for harnesses) and `pg_full_disk_returns_an_error_and_data_stays_readable` (needs a small disk). None of them cover sessions, though `pg_bad_cases` runs against this schema. At landing, merged with spec 59, `pg_bad_cases` passed alone on a fresh database with nothing else on the server, and the 10 selected Postgres tests and all workspace checks passed again.

`pg_kill_jobs_200_times` first failed here: its cleanup deletes tokens, and sessions blocked that. Sessions now cascade with their token and principal, shown in SQL and by that test passing with a session row present.

**Decisions**
- 12 hours, not extended by use. Sign-in failures are counted in memory, by peer address, since a forwarded address can be forged. So behind a proxy all clients share one count, and one client can lock out the rest for a minute.
- TLS belongs to a proxy, so `X-Forwarded-Proto: https` is what makes the cookie `Secure`. A proxy that doesn't send it leaves the cookie without `Secure`.
- The cookie's path is `/api/v1`, since nothing else reads it.
- Sign-in needs the custom header too, which stops another site signing a browser in to the attacker's account.
- `build.assetsInlineLimit: 0` in Vite, as inlined `data:` fonts broke the CSP.
- `GET /nope` now returns the console, as this spec says, so an existing test moved to `/api/v1/nope`.

**Open: inline styles.** Radix's modal scroll lock, in the dialog and command palette, adds a `<style>` element that `default-src 'self'` blocks. Each modal opening logs one console error, and `body` stays scrollable: a wheel over the page behind the palette scrolled it 300 px, though the dialog held still. The spec's strict CSP stays, and `e2e:session` allows that one message by name. The other way is `style-src 'self' 'unsafe-inline'`, which removes the error and weakens the CSP. For the user to decide, ideally before spec 040 builds on modals.

**Follow-ups.** Spec 040 has to send `x-memex-csrf` and call the sign-in route. Spec 038's stream route should use `authed`, which gives it cookies.
