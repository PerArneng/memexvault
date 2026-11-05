# AGENTS.md

Guidance for AI coding agents working in this repository.

## Project

memexvault is a knowledge base, or "second brain", for AI agents.

The repository holds this file, the license, the spec-driven workflow, and no
product code yet. The backend is planned in specs 002-032, and
[docs/architecture.md](docs/architecture.md) holds their shared design.

Decided: the backend is Rust, structured by the `rust-architecture` skill, with an
HTTP API and MCP. PostgreSQL, with pgvector allowed, is the system of record and
the one required external system. Blobs live in folders on disk. AI calls are the
only other accepted network dependency. Backends are swappable behind traits, and
code above the edge traits must not assume a particular backend. The spec tooling's
use of Python and `uv` doesn't decide anything for the product.

`research/` holds background reports converted from PDF (spec 001). It's git-ignored
and exists only locally, so never commit it or copy passages from it. The reports
are source material, not decisions: treat their claims as unverified.

## Start here

Run `uv run .claude/skills/spec-driven/scripts/spec-manager.py list` to see what
is in progress and what is next. Respect each spec's `Depends on` line: spec 032,
the Postgres store, comes right after 009 despite its id.

## Working in this repository

- Follow `docs/architecture.md`. If a task depends on a decision it doesn't cover,
  ask rather than choosing one silently.
- Keep changes small and limited to the task at hand.
- Update this file when a decision changes what it says. As the project takes
  shape, add the real build, test, and lint commands here.

## Spec-driven development

All non-trivial work is done from a spec. That covers new features, design and
technology decisions, changes to behavior, and tooling. Fixing a typo or a small
note doesn't need a spec. The `spec-driven` skill
(`.claude/skills/spec-driven/SKILL.md`) has the full workflow. Its script
`.claude/skills/spec-driven/scripts/spec-manager.py` is the only way to create and
move specs, and it finds `<repo>/specs` from any directory.

No build or test commands exist yet, so each spec's Verification section states
its own concrete check. Write down what was measured, not just that it was done.
Run `check` before committing spec changes, and name the spec in the commit
message (`spec 001: ...`).

## License

Apache License 2.0. See [LICENSE](LICENSE).
