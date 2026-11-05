---
name: rust-architecture
description: Trait-fronted, constructor-injected Rust architecture - traits in front of every module with an edge behind it, IO confined to thin edge modules, pure crates that cannot name the async runtime / network / external services, one composition root, a clippy disallowed-methods lint for the IO rule, and happy plus bad-case tests per pure module that run in memory. Use when structuring, refactoring or reviewing Rust code toward testable, IO-free modules, or when adding a feature to a Rust codebase that follows it.
---

# Trait-fronted Rust architecture

This skill is **reference information**, not a script. It describes one consistent way to structure
a Rust workspace. Load it, then decide what to do with it: scaffold, refactor toward it, or review
against it.

The goal it optimises for is **testability without IO**: every piece of logic can run under
`cargo test` in memory, with no clock, no environment, no files, no socket and no external service, because the
few places that touch the outside world sit behind traits and are injected in.

---

## 1. Principles

1. **A trait in front of every module with an edge behind it.** Consumers depend on the trait, as a
   generic parameter (`Engine<D: Device, C: Clock>`), so dispatch is static and monomorphized. Pure
   modules with no edge behind them need no trait: their functions and types are the interface.
2. **Modules are free from IO.** Only designated *edge* modules touch the outside world. Everything
   else takes values in and returns values out.
3. **Everything is constructor-injected.** No module builds its own collaborators or reads a global.
   No `static` / `OnceLock` / `LazyLock` holding config or mutable state, no `std::env::var` in
   library code. Constant tables are fine: they are data, not state.
4. **The type system and the linter do the checking.** `cargo clippy -D warnings`, plus a
   `clippy.toml` `disallowed-methods` list that turns the IO rule into a compile-time lint.
5. **Testing is a design goal.** Pure modules get unit tests (happy path *and* bad cases); the
   whole core loop runs in memory against fakes.
6. **Frontends are thin.** HTTP handlers and CLI subcommands parse input, call the core, format
   output. No business logic.
7. **Interfaces and implementations are separated by crate boundaries.** The dependency direction
   goes one way, and Cargo enforces it: a crate that does not list `reqwest` or `tokio` in its
   `Cargo.toml` *cannot* call a service or do async IO.
8. **One concept per module file.** The file name says what is in it (`clock.rs`, `admission.rs`).
   A trait with its small data types, or a type with its private helpers, belong together; Rust
   does not need one-type-per-file. A file that mixes policy, edge calls and globals gets split.

---

## 2. Layout

```
Cargo.toml                 # [workspace], [workspace.lints]
clippy.toml                # disallowed-methods / disallowed-macros (the IO rule)
crates/
  <app>-types/             # pure: constants and plain data shared by core and edges (no deps)
  <app>-core/              # pure: traits, data types, policy modules, the facade loop
    src/lib.rs             # `pub use` re-exports = the import surface
    src/<trait>.rs         # one trait + its data types (clock.rs, llm.rs, inbox.rs)
    src/<policy>.rs        # pure functions on plain data, with #[cfg(test)] mod tests
    tests/                 # facade integration tests against in-memory fakes
  <app>-api/               # pure: wire-format translation (JSON in, params out), no web framework
  <app>-llm|db|...         # edge crates: the heavy IO dependency lives only here
  <app>/                   # binary: main.rs (composition root), frontends, edge impls of the traits
```

**Dependency direction, strictly one way:**

```
main.rs (composition root) -> frontends -> pure core -> traits + data types
                              edge impls ------------> traits + data types
```

The pure crates never depend on an edge crate, the async runtime or the web framework. Check with
`cargo tree -p <pure-crate>`.

Where the trait implementation for an edge type lives is decided by the orphan rule: implement the
core trait in the binary crate (a wrapper struct around the edge type) rather than make the edge
crate depend on the core crate.

---

## 3. Naming

- Traits are the **plain noun**: `Llm`, `Clock`, `Inbox`. No `I` prefix, no `Trait` suffix.
- Implementations carry a **qualifying prefix**: `AnthropicLlm`, `SystemClock`, `MpscInbox`, and in
  tests `ScriptedLlm`, `ManualClock`, `ScriptedInbox`.
- Each crate's `lib.rs` re-exports its public names with `pub use`; callers write
  `use app_core::{Clock, Engine}`, not deep module paths.

---

## 4. Edge modules

An edge module's whole job is to talk to something outside the process:

| Edge | Typical implementation | Why |
|---|---|---|
| clock | `SystemClock` (`Instant::now`) | non-deterministic, so injectable |
| randomness | `SystemSeed` | seeds from the time, random ids |
| environment | `Settings::from_env` called only from `main.rs` | env vars and CLI args |
| file system | loaders, dumps | real files |
| network | the web framework's handlers | HTTP |
| channels / threads | `MpscInbox`, an event-sink wrapper, `thread::spawn` | blocking receives wait on wall time |
| external services | LLM / text-to-speech clients | remote calls, latency, cost |

**The clock is the one people forget.** `Instant::now()` or `.elapsed()` inside a policy makes it
untestable; so does `recv_timeout`, which waits on wall time. Time arrives through `Clock::now()`;
waiting goes through a small trait (`try_recv`, `recv_timeout(d)`) whose fake advances a manual clock.
Pull the *decision* out as a pure function ("given elapsed, gauge, what the last receive returned:
wait how long, or stop?") and drive it from the loop.

**The rule, enforced** with a workspace `clippy.toml`:

```toml
disallowed-methods = [
  "std::env::var", "std::env::var_os", "std::env::vars", "std::env::args",
  "std::time::Instant::now", "std::time::Instant::elapsed",
  "std::time::SystemTime::now", "std::time::SystemTime::elapsed",
  "std::thread::sleep", "std::sync::mpsc::Receiver::recv_timeout",
  "std::fs::read", "std::fs::read_to_string", "std::fs::read_dir", "std::fs::write",
  "std::fs::File::open", "std::fs::File::create",
]
disallowed-macros = ["std::println", "std::eprintln", "std::print", "std::eprint", "std::dbg"]
```

`clippy.toml` applies to the whole workspace, so edge crates and edge files opt out with a
crate- or module-level `#![allow(clippy::disallowed_methods)]` (and `disallowed_macros`) plus a
one-line reason. Edge implementations stay thin: they translate and delegate. Any logic worth
testing moves into a pure module that receives the edge through its trait.

`tracing` macros are fine everywhere: `tracing` already is the logging abstraction, and the
subscriber (the real output edge) is installed only in `main.rs`. No `Logger` trait.

---

## 5. Data types

- Plain `struct`s and `enum`s next to the trait or policy that uses them, `#[derive(Debug, Clone,
  PartialEq)]` where it makes sense; `serde` only on types that cross a wire.
- Values are immutable unless declared `mut`: pass `&T` or move. No interior mutability
  (`Cell`/`RefCell`) in data types.
- Data only: no IO, no references to edge types. Use std types (`Duration`, `PathBuf`, an
  associated `Instant` type on the `Clock` trait) instead of wrappers. Newtypes only where they
  prevent a real mix-up.
- **Settings:** every knob that is an env var becomes a field of a settings struct with `Default` =
  today's defaults. Parse from a lookup function, `Settings::from_vars(|name| ...) -> Result<Self>`,
  so parsing is pure and tested. The one-line edge that passes `std::env::var` (`fn env_var(name) ->
  Option<String>`) lives in `main.rs`, which calls `from_vars(env_var)` once at startup: the pure
  crates then contain no env call at all, and need no lint allow. Keep existing names, defaults and parsing quirks exactly when
  refactoring (pin each quirk with a test); reject values that would panic later with a startup
  error instead.

---

## 6. The facade

One type - conventionally `Engine` - is the single entry point to the core: generic over its edges,
with a `run(...)` or one method per use case. Frontends translate and call it. A new capability
becomes a facade (or api) method first; the frontend then only learns how to invoke it. If the
generic parameter list gets long, group the edges into one `Edges` trait with associated types
rather than reach for a global or `Box<dyn>`.

---

## 7. Dependency injection

Manual constructor injection; no DI container crate. `main.rs` is the container:

```rust
let settings = Settings::from_env()?;                       // the only env read
let llm = AnthropicLlm::new(LlmSettings::from_env()?)?;
let gauge = Arc::new(AtomicGauge::default());              // shared state, created once, handed out
let engine = Engine::new(llm, SystemClock, SystemSeed::new(), gauge.clone(), settings);
```

- **Exactly one composition root.** Nothing else constructs a system edge or reads env.
- Several edges of one consumer travel together as a plain struct (`EngineEdges { clock, seed, gauge, stats,
  settings }`) rather than a long `new(...)` argument list. A type parameter that only some calls need (the event
  sink of the requests a `run` call receives) goes on that method, not on the struct.
- Global mutable state (`static AtomicUsize`, `static SEED`) becomes a value created in `main.rs` and
  passed by `Arc` to every user, or state of the edge implementation that owns it.
- **Tests never use the production wiring.** They call `Engine::new(ScriptedLlm::..., ManualClock::...)`.

---

## 8. Testing

- **Unit tests** sit next to the code (`#[cfg(test)] mod tests`) and cover both the **happy path**
  and the **bad cases**: empty or edge-of-range input, invalid values, limits hit, a receiver gone,
  malformed input. A pure module is done when both kinds are in place.
- **Integration tests** of the facade live in `crates/<core>/tests/` and run the whole loop through
  fakes.
- **Fakes over mocks.** A fake with real behaviour (a page allocator, a scripted LLM reply, a
  `Vec` that collects events and can be closed) catches integration mistakes that call-count
  assertions never will.
- Because clock and seed are injected, assertions hardcode times and random draws. No sleeping, no
  flakiness.
- Tests only link where the dependency tree does: a crate that pulls a native library the dev machine
  lacks cannot run its tests there. Put testable code in crates without it.

---

## 9. Tooling

```
cargo test -p <pure crates>          # runs anywhere, no network or services
cargo clippy --workspace -- -D warnings
cargo fmt --check                    # if the repo uses rustfmt
cargo tree -p <pure crate>           # must not list the edge crates
```

Pre-existing style lints (`too_many_arguments`, `type_complexity`, ...) on a large codebase can be
allowed by name in `[workspace.lints.clippy]` so `-D warnings` enforces the disallowed lists and
correctness lints without a churn commit; record the choice.

---

## 10. Checklist for adding a feature

1. Is it policy or IO? Policy goes into a pure crate as functions on plain data; IO goes into an edge.
2. New edge? Add the trait (plain noun) with its data types in the core crate, the system
   implementation in the binary crate, and a fake in the tests.
3. New setting? A field on the settings struct with its default, parsed in `from_vars`, with a test.
4. Wire it in `main.rs` only.
5. Expose it as a facade method; the frontend only calls it.
6. Happy and bad-case unit tests for the pure part; an integration test through the fakes if the loop
   changes.
7. `cargo clippy --workspace -- -D warnings` (the disallowed lists catch IO that crept into a pure
   crate), `cargo test` on the pure crates.

---

## 11. When not to use this

The indirection costs a trait, an implementation, a wiring line and a fake per edge. It pays off
for code that grows, gets tested and outlives its author; not for throwaway tools, a single-file
binary that will never get a second frontend, or a thin wrapper around one library. On an existing
codebase, go incrementally, edges first (settings, clock, globals), since that alone makes most of
the code testable.

Do not push traits into hot inner code that has no edge: no abstraction over a stroke renderer's
inner loop, no `dyn` in a hot loop. The trait boundary sits at the granularity the caller already uses.

---

## This repo

This repo has no Rust code, and having this skill here doesn't choose Rust. It applies only once a
spec has made that decision (see `AGENTS.md`).
