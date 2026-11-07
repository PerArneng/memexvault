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
Kept. `memexvault` reaches Postgres over TLS when the URL asks for it, or when the host isn't on this machine, and adds no system library.

**Built.**
- `crates/memex-store-postgres/src/tls.rs` takes `sslmode` and `sslrootcert` out of the URL, in both the URL and `key=value` forms, since the `postgres` crate rejects `verify-full` and `sslrootcert`. It parses the rest once in `PgStore::connect` and opens every connection from that. All connections go through this one place.
- `disable` is plaintext, `require` encrypts and checks no certificate, and `verify-full` checks the CA, the validity dates and the host name. With no `sslmode`, a local host gets `disable` and anything else gets `verify-full`. There's no plaintext fallback: `require` and `verify-full` fail if the server has no TLS.
- `compose.yaml` has a `postgres-tls` service under the `tls` profile on 127.0.0.1:55459. `scripts/pg-tls-certs.sh` writes throwaway certificates to the git-ignored `target/pg-tls/`, so no key is committed. `crates/memexvault/tests/postgres_tls.rs` has 8 tests against real servers.
- Connection errors now include their cause. Before, a bad certificate read only "error performing TLS handshake". `tls_` unit tests and `pg_bad_cases` cover that no message shows the password.

**Measured,** on a busy machine shared with two other workers; the times are only a rough guide.
1. `verify-full`: passed. It connects with the right CA, and `pg_stat_ssl` shows `ssl = true` and TLSv1.3. The wrong CA is refused with `invalid peer certificate: UnknownIssuer`. A host name not in the certificate is refused with `certificate not valid for name "other.memex.test"`. `memexvault migrate` does the same: "schema is up to date" with the right CA, exit 2 with the wrong one.
2. A non-local URL with no `sslmode` refuses plaintext: passed. The URL names `db.memex.test` and uses `hostaddr=127.0.0.1` to reach the plaintext server. It connects with an explicit `sslmode=disable`, so the server is reachable. With no `sslmode` it's refused with `server does not support TLS`, or before any network traffic if `sslrootcert` is missing. The same default connects to the TLS server with the right CA and is refused with the wrong one.
3. `otool -L target/release/memexvault`: passed. It lists `libc++`, `libiconv`, `libSystem`, `libobjc` and the CoreGraphics, CoreFoundation, SystemConfiguration, Foundation and CoreML frameworks: all system libraries, and no OpenSSL.

`cargo test -p memexvault --release --test postgres_tls -- --ignored --test-threads=1`: 8 passed in 0.16 s. `crypto` comes from `ring`, already in the tree through `ureq`, and `Cargo.lock` gains 11 crates, all passing `cargo deny check licenses`. The default service is unchanged, and `docker compose config --services` still lists only `postgres` without the profile.

**Postgres suite.** `pg_vector_contract`, `pg_vector_model_move_keeps_serving`, `pg_vector_rebuild_needs_no_embedder`, `pg_bad_cases`, `pg_batched_lookups`, `pg_group_commit_throughput`, `pg_kill_jobs_200_times`, `pg_kill_memexvault_1000_times`, `pg_registry_contract` and `pg_replay_of_100k_events_matches_mem_store` passed. `pg_contract` passed on a fresh database (see the follow-ups for why it needs one, and why one parallel run failed with `LeaseLost`). Not run: `pg_kill_postgres_100_times`, which would kill a container shared with other workers. Stop before testing anything, as spec 019 records: `pg_child` and `pg_job_child`, which are helpers, and `pg_full_disk_returns_an_error_and_data_stays_readable`, which needs a Postgres on a small disk.

**Decisions.**
- `verify-full` needs `sslrootcert`, and nothing falls back to built-in or system roots, so a default can never trust more than the operator named. A hosted Postgres with a public CA can point it at a bundle such as `/etc/ssl/cert.pem`.
- `sslrootcert` with `disable` or `require` is refused rather than ignored, as the operator would think the certificate was checked. libpq instead verifies under `require` in that case.
- `prefer`, `allow` and `verify-ca` are refused. The spec names three modes, and the first two can silently end up in plaintext.
- `sslmode=disable` stays allowed for a non-local host, as it's explicit.
- Local means `localhost` or a loopback address or a Unix socket, for every host and `hostaddr` in the URL. A host with a loopback `hostaddr` still counts as non-local, which errs towards TLS.
- The CA file is read once at startup, so a new CA needs a restart.
- rustls needs a CA and a separate server certificate with the name in its subject alternative names. It ignores the common name and refuses a self-signed certificate that acts as its own CA, so the compose profile's "self-signed server" is a server certificate signed by a throwaway self-signed CA.

**Follow-ups,** both about existing tests, neither changed here:
- `pg_bad_cases` terminates every advisory-lock holder on the server (`pg_tests.rs:227`), not only in its own database, so parallel runs on one server kill each other's writers. That's what caused the `LeaseLost` failure above. It should filter on `datname = current_database()`.
- `pg_contract` isn't repeatable: its cleanup list leaves out the `sections` and `gone` vaults that `contract::run_all` uses, so a second run on the same database fails with `SequenceGap { head: Some(0), got: 0 }`.
