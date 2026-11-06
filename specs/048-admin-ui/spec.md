---
id: 048
name: admin-ui
description: Build vault, token, status, operations, erasure and MCP administration screens
---

# 048 admin-ui

## Goal
An operator can run the system from the console: vaults, tokens, status, maintenance and erasure, without needing the command line once the server is up.

## Background
**Depends on:** 039, 040. Design: `docs/architecture.md`, sections API conventions, Backends.

## Approach
- Vault create, rename and delete. Principals and tokens with scopes and expiry, where the token's value is shown once with a copy button.
- Status page for system health, Postgres and LLM reachability, lag, queue depth, sizes and caches, updating live.
- Operations: reindex, rollback, reconcile with its report, export, import and sweep, showing job progress.
- Erasure: an impact preview, a typed confirmation of the source's name, and approval through review, followed by a report of what was removed.
- Per-vault chat setting showing the model and endpoint that vault content would go to. Effective settings and backends, read-only. An MCP page with endpoint, tool list and a ready-to-copy client config.

## Verification
1. Vault, principal and token journeys pass in Playwright, and a revoked token stops working at once.
2. A new token's value is never shown again after the dialog closes, and appears in no later API response or log.
3. Secrets are shown as redacted on the settings page, using the markers from spec 039.
4. An erasure from the UI shows a preview that matches the result, and needs human approval.
5. Destructive actions can't run with a single click, shown by a test of every one.
6. Every admin screen shows a clear permission message for a non-admin principal, and the API refuses it as well.
7. With Postgres down, the status page explains the problem instead of failing.

## Out of scope
Runtime editing of settings, user self-registration and billing.

## Result
Not started.
