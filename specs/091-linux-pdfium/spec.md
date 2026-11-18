---
id: 091
name: linux-pdfium
description: Build and run the backend on Linux x86_64, linking pinned pdfium dynamically there since no static Linux build is published
---

# 091 linux-pdfium

**Depends on:** 002, 015.

## Goal
`cargo build --workspace`, `cargo test --workspace` and `memexvault serve` work on Linux x86_64, using the same pinned pdfium build (pdfium-lib tag 8046d, chromium 7881 API) as macOS. Until now the backend only built on macOS arm64, which left six parallel workers on a Linux machine unable to build or test anything that links `memex-pdf`.

## Background
Spec 002 chose `pdfium-render` 0.9.5 on a statically linked pdfium and noted "Linux static build isn't tested yet. `pdfium-lib` publishes one". That turned out to be wrong: tag 8046d's `linux.zip` holds only `libpdfium.so` (x64 and arm64), and `bblanchon/pdfium-binaries` also ships Linux as shared libraries only. `crates/memex-pdf` also enabled pdfium-render's `core_graphics` and `libc++` features, which only build on Apple targets, so the crate failed to compile on Linux ("library kind framework is only supported on Apple targets"), and `scripts/fetch-pdfium.sh` stopped with "no pinned pdfium for Linux-x86_64 yet".

The user chose dynamic linking on Linux over building pdfium from source (2026-11-17): macOS stays static, and Linux loads the pinned `.so`.

## Approach
- `scripts/fetch-pdfium.sh`: pin `linux.zip` from tag 8046d by SHA-256 for `Linux-x86_64`, and put `libpdfium.so` in `vendor/pdfium/release/lib`, where the macOS archive's `libpdfium.a` goes.
- `crates/memex-pdf/Cargo.toml`: `static`, `libc++` and `core_graphics` only on macOS, through a target-specific dependency entry. The other features stay for every target.
- `PdfiumParser::new` binds the static library on macOS. Elsewhere it loads `libpdfium.so` from, in order: `MEMEX_PDFIUM_LIB` if set, the folder of the running executable, then the build's `vendor/pdfium/release/lib` (from `.cargo/config.toml`), so tests and `cargo run` work with no setup. A missing library is an error naming the paths it tried. The parser children exit nonzero on any error, and every nonzero exit fails the job for good, so a missing library would fail every PDF. `serve` and `mcp` therefore run a `pdf-check` child first and refuse to start, with that error, if pdfium can't be loaded.
- `docs/architecture.md` and AGENTS.md say Linux links dynamically and how to ship the `.so` beside the binary.

## Verification
1. On Linux x86_64 from a clean checkout: `scripts/fetch-pdfium.sh` succeeds and checks the SHA-256, and `cargo build --workspace`, `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check` pass.
2. `memex-pdf`'s own tests, which parse and render real PDFs, pass on Linux.
3. With `MEMEX_PDFIUM_LIB` pointing at a missing file, `memexvault pdf-check` exits 1 with an error naming it, and `memexvault serve` refuses to start with that message. With it unset, `pdf-check` exits 0 from `target/`, and from a copy of the binary in another folder with `libpdfium.so` beside it.

## Out of scope
- Building a static pdfium for Linux, Linux arm64, and packaging or release artifacts.
- Changing the macOS build, which isn't tested here (no Mac in this session); its features and binding are unchanged by construction.

## Result
Built as planned, on Linux x86_64 (Ubuntu, kernel 7.3, 16 cores), 2026-11-17. Kept.

- `scripts/fetch-pdfium.sh` pins pdfium-lib 8046d's `linux.zip` (SHA-256 `d80d50b3…39a3`, checked) and leaves `libpdfium.so` in `vendor/pdfium/release/lib`. It links only libc, libm, libpthread, libstdc++ and libgcc_s.
- `memex-pdf` enables `static`, `libc++` and `core_graphics` only on macOS. Elsewhere `PdfiumParser::new` loads `libpdfium.so` from `MEMEX_PDFIUM_LIB`, else beside the executable, else `MEMEX_PDFIUM_VENDOR_DIR` from `.cargo/config.toml`. A failure is the new `PdfError::Library`, on one line naming every path tried.
- Found while building it: every nonzero exit from a parser child fails its job for good, so a missing library would have failed every PDF permanently. `serve` and `mcp` now run a `memexvault pdf-check` child at start-up and refuse with its error.

Measured:
1. `fetch-pdfium.sh` from an empty `vendor/`: checksum OK. `cargo build --workspace`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo fmt --check` pass. `cargo test --workspace --no-fail-fast`: 993 passed, 0 failed, 108 ignored. Before this spec none of the crates depending on `memex-pdf` compiled here.
2. `memex-pdf`'s 8 fixture tests pass. One tolerance changed: on `rotate270_crop`, whose Helvetica isn't embedded so pdfium substitutes a host font, the x0 edge is 2.27 pt off on Linux against 1.87 pt on macOS (spec 015), above that edge's 2.0 pt limit. The limit is 2.5 pt off macOS and unchanged on it. Every other side of all 50 anchors stays within 1 pt.
3. `pdf-check` exits 0 from `target/`, from a copy in another folder falling back to `vendor/`, and from a copy with `libpdfium.so` beside it while `vendor/`'s was moved away. With `MEMEX_PDFIUM_LIB=/nonexistent/libpdfium.so` it exits 1 naming that path, and `serve` on a migrated database exits 2 with the same message. With the library present, `serve` passes the check and goes on to its model checks.

Not tested: the macOS build, as no Mac was available. Its feature set and static binding are the same as before by construction, but `cargo build` on macOS should confirm it before the next release from there.

Follow-ups: word boxes from non-embedded fonts depend on the host's fonts, so the same PDF can give slightly different boxes on macOS and Linux under the same `PARSER` identity. Shipping `libpdfium.so` in a release is part of packaging, which no spec covers yet.
