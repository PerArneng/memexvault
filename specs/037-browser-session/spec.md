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
Not started.
