---
name: adk-rust-docs
description: Look up how to use ADK-Rust (zavora-ai/adk-rust, the Rust Agent Development Kit - crates adk-rust, adk-core, adk-agent, adk-model, adk-tool, adk-runner, adk-realtime, adk-graph, adk-session, adk-server, adk-anthropic, adk-gemini and the rest) from its own docs and examples, by keeping a shallow clone in the system temp dir and searching it. Use this whenever you need to know how ADK-Rust works or how to write code against it - LlmAgent, Runner, tools and the #[tool] macro, model providers (Gemini, OpenAI, Anthropic, Ollama), sessions, memory, realtime voice agents, graph or workflow agents, MCP, A2A, the server or launcher, migration between versions - or before adding, upgrading or debugging an adk-* dependency, even if the user only says "the agent framework", "adk" or "how do I make the agent speak / call a tool". Prefer it over answering from memory: the API moves fast and changed a lot between 1.x, 2.x and 3.x.
---

# ADK-Rust docs lookup

ADK-Rust changes quickly (3.0 broke a lot of 2.x code), so answers from memory are often wrong or
outdated. The repo itself carries the authoritative docs and runnable examples. This skill keeps a
local shallow clone and searches it, so every answer can point at a real file.

## 1. Make sure the clone exists and is current

The clone lives in the **system temp dir** (`$TMPDIR` on macOS, `/tmp` elsewhere) in a folder named
`adk-rust`. Run this before searching; it clones on first use and fast-forwards afterwards. A failed
update (offline) is fine: the old clone is still usable, just say it may be stale.

```bash
ADK="${TMPDIR:-/tmp}"; ADK="${ADK%/}/adk-rust"
if [ -d "$ADK/.git" ]; then
  git -C "$ADK" pull --ff-only --quiet || echo "WARN: could not update $ADK, using existing clone"
else
  git clone --depth 1 --quiet https://github.com/zavora-ai/adk-rust.git "$ADK"
fi
echo "$ADK"; git -C "$ADK" log -1 --format='%h %cd'; grep -m1 '^version' "$ADK/Cargo.toml"
```

Use the printed absolute path in later commands (the shell does not keep variables between calls).
The clone is a read-only cache: never edit, build or commit in it (building pulls gigabytes into
`target/` on a nearly full disk).

**Version match.** The clone is `main`, which may be ahead of the crate version the project uses.
Compare the workspace `version` printed above with the `adk-*` version in the project's
`Cargo.toml`/`Cargo.lock`. If the major or minor differs and the answer depends on the API, look at
the matching tag in a separate folder instead of moving the cache:
`git clone --depth 1 --branch vX.Y.Z https://github.com/zavora-ai/adk-rust.git "${TMPDIR:-/tmp}/adk-rust-vX.Y.Z"`
(tags are `v1.0.0`, `v2.3.0`, ...). For upgrades read `docs/official_docs/migration/`.

## 2. Where to look

Search in this order; stop as soon as you have a confident, file-backed answer.

1. **`docs/official_docs/`**: the user guide. Start at `docs/official_docs/index.md`, a linked table
   of contents with one line per page. Folders by topic: `core/` (Content, Part, Agent and Tool
   traits, Runner, plugins), `models/` (providers), `agents/` (LlmAgent, workflow, multi-agent, graph,
   realtime, desktop audio), `tools/`, `sessions/`, `memory/`, `realtime/` (voice agents),
   `mcp/`, `a2a/`, `deployment/` (server, launcher), `callbacks/`, `events/`, `artifacts/`,
   `security/`, `migration/`, `quickstart.md`.
2. **`examples/`**: runnable crates, one folder each with `README.md`, `Cargo.toml` and `src/`.
   `examples/README.md` describes many of them. Most examples now live in the separate
   `zavora-ai/adk-playground` repo (see below); those left here are mostly live validation crates.
3. **Per-crate examples and READMEs**: `adk-*/examples/*.rs` (for instance `adk-rust/examples`,
   `adk-agent/examples`, `adk-realtime/examples`, `adk-model/examples`) and `adk-*/README.md`. These are
   often the shortest complete program for one feature.
4. **Source, for exact signatures**: `adk-*/src/`. Use it to confirm a builder method, feature flag
   (`adk-*/Cargo.toml` `[features]`) or type when the docs are vague. `docs/design/` holds design specs,
   only useful for the why.

Useful searches (the Grep tool works too; `rg` may not be installed, so plain `grep` is shown):

```bash
A=/path/to/adk-rust   # the path printed in step 1
grep -rl "LlmAgentBuilder" $A/docs $A/examples                       # which files cover it
grep -rn -C3 -E "\.tool\(|tools\(" $A/docs/official_docs/agents       # matches with context
grep -n "^#" $A/docs/official_docs/realtime/index.md                  # page outline
ls $A/examples | grep -i realtime; ls $A/adk-*/examples                # find examples by name
```

Read the matching doc page and at least one example before writing code: the docs explain intent,
the examples show the imports, feature flags and builder chains that actually compile.

If nothing local covers the topic, the playground repo has 120+ more examples. Clone it the same way
(`zavora-ai/adk-playground` into `${TMPDIR:-/tmp}/adk-playground`, shallow) and search it, or tell the
user it exists.

## 3. How to answer

- Cite the files you used with paths relative to the clone (e.g.
  `docs/official_docs/agents/llm-agent.md`, `examples/realtime_voice/src/main.rs`), and the clone's
  commit and version from step 1, so the user can tell how fresh the answer is.
- Copy the real imports, crate names and feature flags from the example; do not invent builder
  methods. If the docs and the source disagree, trust the source and say so.
- When the project follows the `rust-architecture` skill, keep to it here too: ADK-Rust
  types that do network IO (models, realtime sessions, servers) belong in edge modules behind a
  project trait, wired up in the composition root.
- If the clone has nothing on the question, say that plainly rather than filling the gap from memory;
  mark anything not backed by a file as unverified.
