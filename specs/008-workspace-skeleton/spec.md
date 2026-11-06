---
id: 008
name: workspace-skeleton
description: Create the Rust workspace with crate boundaries and the IO lint enforced from the first commit
---

# 008 workspace-skeleton

## Goal
A compiling workspace with the crates from `docs/architecture.md`, where the architecture rules are enforced by Cargo and
clippy, not by convention. A binary that starts and prints its version.

## Background
**Depends on:** 002-007, because their results decide the edge crates' dependencies. Design: `docs/architecture.md`,
section Crates, and the `rust-architecture` skill.

## Approach
- Root `Cargo.toml` with workspace lints, `clippy.toml` with the skill's disallowed lists, and a pinned toolchain.
- All crates in the layout, mostly empty. Pure crates list no runtime, network or web dependency.
- `Clock` and `Seed` traits with `SystemClock`, `ManualClock` and a seeded fake.
- `Settings::from_vars` in a pure crate, with the one env read in `main.rs`. `tracing` subscriber installed in `main.rs`.
- A `[backends]` setting that accepts the default names only for now, so choosing a backend is a config decision from the
  start. See `docs/architecture.md`, section Backends.
- Licence check with `cargo-deny`, as a dev tool, denying AGPL and GPL.
- Add the real build, test and lint commands to `AGENTS.md`.

## Verification
1. `cargo build --workspace`, `cargo test --workspace`, `cargo clippy --workspace -- -D warnings` and `cargo fmt --check`
   pass.
2. `cargo tree -p memex-core` and `-p memex-types` list no `tokio`, `axum`, Postgres client, `tantivy` or ADK crate.
3. A temporary `std::fs::read` in `memex-core` fails clippy and `Instant::now` does too. Revert both and record the output.
4. `cargo deny check licenses` passes, and a test run against a denied licence fails.
5. Settings tests cover defaults, valid values and bad values, including an unknown backend name refused at startup.
6. `AGENTS.md` lists the commands.

## Out of scope
Any feature logic, CI hosting and release packaging.

## Result
Built the Cargo workspace with all 16 crates, `clippy.toml` with the skill's lists, `deny.toml`, `rustfmt.toml`, and a toolchain pinned to Rust 1.99.1. `Clock` and `Seed` are in `memex-core`, with `SystemClock` and `SystemSeed` in the binary and `ManualClock` and `SeededRandom` in `memex_core::testing`. `Settings::from_vars` is pure, and `main.rs` holds the only env read. Edge crates are empty until their own specs give them dependencies. Their dependencies are already known from specs 002 to 007.

**Verification.**
1. `cargo build --workspace`, `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check` pass.
2. `cargo tree -p memex-core` and `-p memex-types` list no `tokio`, `axum`, Postgres client, `tantivy` or ADK crate. At spec 009 `memex-core` has 27 crates, all serialization and hashing.
3. Temporary lines in `memex-core` fail clippy: ``error: use of a disallowed method `std::fs::read` ``, ``error: use of a disallowed method `std::time::Instant::now` ``, ``error: use of a disallowed macro `std::println` ``. Reverted, and clippy is clean again.
4. `cargo deny check licenses` prints `licenses ok`. A temporary workspace member with `license = "AGPL-3.0-only"` gives `error[rejected]: failed to satisfy license requirements` and `licenses FAILED`. Removed.
5. Settings tests cover defaults, valid values, an unknown backend naming the choices (`unknown Store backend 'redb', known: postgres`, exit code 2 from the binary), every backend variable, empty values and bad bind addresses. Debug output redacts the database URL, and a run with a password in the URL logs no password.
6. `AGENTS.md` lists the commands.

`memexvault --version` prints `memexvault 0.1.0`. Backends are environment variables (`MEMEX_BACKEND_STORE` and so on), since the spec named no config file. `rust-toolchain.toml` made rustup install 1.99.1 as a named toolchain on first use. Workspace lint `unsafe_code = "forbid"` is on, and an edge crate needing FFI will have to allow it with a reason.
