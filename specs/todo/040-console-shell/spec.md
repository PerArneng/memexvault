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
Not started.
