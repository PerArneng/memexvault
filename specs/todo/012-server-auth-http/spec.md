---
id: 012
name: server-auth-http
description: Serve HTTP with isolated vaults, scoped tokens and attribution on every write
---

# 012 server-auth-http

## Goal
A running server hosting several isolated vaults, where each human or agent token has per-vault read, propose or admin
scope, and every write records who made it.

## Background
**Depends on:** 032, 011. Design: `docs/architecture.md`, sections Decisions, Crates.

## Approach
- `Engine` facade with one method per use case. `memex-http` only parses, calls and formats. DTOs live in `memex-api`.
- A server registry in the `memex_server` schema, separate from any vault: vaults, principals (human or agent), token hashes and scopes.
- Tokens are random, shown once and stored hashed, with revocation and expiry through `Clock`.
- Authorization is a pure function of principal, vault and operation.
- API: vault create and list, token create, list and revoke, whoami, health, version.
- Binds to localhost by default. Body size limits, request ids, a consistent error format and an OpenAPI document.
- A first-run bootstrap creates an admin token once.

## Verification
1. A table-driven test of all scopes × all operations × own and other vault, with no gaps. Every denied case returns the
   right status and leaves no state change.
2. A token for vault A can't detect that vault B exists: same response as for a missing vault.
3. Every write event in an integration test has the correct actor, and no write path exists without one.
4. Bad cases: revoked, expired and malformed tokens; oversized bodies; the token doesn't appear in logs (tested by
   capturing logs).
5. `cargo tree -p memex-core` still lists no axum or tokio.

## Out of scope
TLS (use a reverse proxy if needed), SSO or OAuth, per-object ACLs, and a UI.

## Result
Not started.
