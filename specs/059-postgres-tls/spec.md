---
id: 059
name: postgres-tls
description: Connect to Postgres over TLS when the URL asks for it, without adding a system library
---

# 059 postgres-tls

## Goal
Let `memexvault` reach a Postgres on another machine securely. Today the client uses `NoTls`, which suits only the same machine.

## Background
**Depends on:** 032. Spec 006 chose the `postgres` crate. `rustls` through `tokio-postgres-rustls` should add TLS with no system library, which needs checking with `otool -L`.

**Assumption in force:** no TLS for now. The default is Postgres on the same machine, and the docs say so.

## Approach
- Parse `sslmode` from the URL: `disable`, `require`, `verify-full`.
- Default to `verify-full` for any non-local host.
- A `compose.yaml` profile with a self-signed server for testing.

## Verification
1. `verify-full` connects with the right CA and refuses the wrong one.
2. A non-local URL with no `sslmode` refuses plaintext.
3. `otool -L` lists system libraries only.

## Out of scope
Client certificates.

## Result
(Filled in when the spec moves to done or dropped: what was built, decisions, numbers, follow-ups.)
