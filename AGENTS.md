# AGENTS.md

Guidance for AI coding agents working in this repository.

## Project

memexvault is a knowledge base, or "second brain", for AI agents.

The project is at the very start: the repository holds this file, the license,
the spec-driven workflow, and no product code. No language, framework,
architecture, or storage format has been chosen. The spec tooling's use of Python
and `uv` doesn't decide any of these for the product.

`research/` holds background reports converted from PDF (spec 001). They are source
material, not decisions: treat their claims as unverified.

## Start here

Run `uv run .claude/skills/spec-driven/scripts/spec-manager.py list` to see what
is in progress and what is next.

## Working in this repository

- Don't assume a tech stack or design. If a task depends on a decision that
  hasn't been made, ask rather than choosing one silently.
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
