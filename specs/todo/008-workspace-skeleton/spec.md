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
Not started.
