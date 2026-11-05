---
id: 039
name: admin-ops-api
description: Expose operations, settings, cost and system status over HTTP for the admin console
---

# 039 admin-ops-api

## Goal
Everything an operator does on a running server can be done over HTTP, with progress and reports. Commands that have to run before the server is up stay on the command line.

## Background
**Depends on:** 021, 024, 030. Design: `docs/architecture.md`, sections API conventions, Backends.

## Approach
- Reindex, generation rollback, reconcile, export, import, orphan sweep and compaction as admin-scoped job routes with report retrieval.
  `migrate` and `models fetch` stay CLI-only, because they run before the server can serve anything.
- Effective settings, read-only, with secrets redacted. Active backends, models and their status, and thresholds.
- Status: Postgres and LLM reachability, index lag, queue depth, sizes, memory, and cache stats with a clear-cache action.
- An LLM usage report by agent, model, vault and day, built from Derivation token counts. It shows cost only where a price is configured, and it never
  guesses one.
- MCP connection info: endpoint, transports, protocol version and tool list.

## Verification
1. Each operation runs through its route, reports progress in the jobs API, and gives the same result as the command.
2. A secrets test: a marker set as the database password, LLM key and transcriber key appears in no settings, status, log or error response.
3. The token report matches a sum over Derivations on a fixture vault, and shows no cost when no price is set.
4. Every route is refused without admin scope, shown by a scope matrix.
5. With Postgres or the LLM down, status reports it correctly and promptly, within 2 s.
6. Bad cases: starting a second reindex of the same kind is refused with a clear error, and restoring over an existing vault needs the explicit flag.

## Out of scope
Runtime editing of settings, external monitoring integrations, and billing.

## Result
Not started.
