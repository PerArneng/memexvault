---
name: spec-driven
description: Spec-driven development with a specs/ work queue (todo / working / hold / done / dropped) managed by the bundled spec-manager.py. Use this whenever starting any non-trivial piece of work (a new role, recipe, feature, tool, experiment or machine change), when the user says "make a spec", "add this to the queue", "what's next", "pick up the next task", "park this", "drop that idea", when finishing or abandoning work, when recording work that was done without a spec, or when adopting spec-driven development in a repo, even if the user doesn't say "spec".
---

# Spec-driven development

Every non-trivial piece of work starts as a spec: a short `spec.md` that says what will change, why, how, and how
to tell whether it worked. The `specs/` tree is the work queue. A spec's status is the folder it lives in, so
`ls specs/working` always answers "what is in progress", and a new session can pick up exactly where the last
one stopped. Specs that were dropped stay in the tree with their reason, so nobody tries the same dead end twice.

The bundled script is the only thing that creates or moves specs. It numbers them, keeps headers and folder
names consistent, and refuses to change anything while the tree is invalid. Never create, number or move spec
folders by hand.

```bash
SM=".claude/skills/spec-driven/scripts/spec-manager.py"   # run from anywhere in the repo; it finds <git root>/specs
uv run $SM list              # what is in todo / working / hold / done / dropped
uv run $SM check             # validate only; run before committing spec changes
```

## Layout

```
specs/
  todo/  working/  hold/  done/  dropped/   # status = the folder a spec lives in (.gitkeep keeps empty ones)
    NNN-kebab-name/        # e.g. 001-open-webui-role; NNN = 001, 002, ... never reused
      spec.md              # the spec (required)
      *.mmd, *.dot, ...    # optional supplementary files (diagrams, data, notes)
  ideas/                   # one file per idea for a future spec; not tracked by the script
```

`spec.md` starts with a YAML header with exactly these keys, matching the folder name:

```
---
id: 001
name: open-webui-role
description: One line saying what the spec achieves
---
```

It is followed by these sections. `init` writes them as a skeleton with hints:

| Section | What goes in it |
|---|---|
| **Goal** | What changes, and why it matters. Give numbers where they exist. |
| **Background** | Context, measurements, user requests, links to docs and earlier specs. |
| **Approach** | Files, components and steps. |
| **Verification** | The tests and the concrete check that decide keep or drop. |
| **Out of scope** | What the spec deliberately leaves out. |
| **Result** | Filled in at the end: what was built, numbers, decisions, follow-ups. |

## Commands

Every command first validates the whole tree. If any spec is malformed, it changes nothing and prints every
problem at once. `--dry-run` on `init`, `working`, `done`, `hold`, `resume` and `drop` shows what would happen.

| Command | Effect |
|---|---|
| `init NAME "DESCRIPTION"` | Creates `todo/NNN-NAME/spec.md` with the next id and prints its path. Fill in every section before starting. |
| `working ID` | Moves a spec from todo to working. It is refused while another spec is in working. |
| `done ID` | Moves a spec from working to done. |
| `hold ID "REASON"` | Moves a spec from todo or working to hold. It writes `> **On hold (YYYY-MM-DD):** REASON` under the title. |
| `resume ID` | Moves a spec from hold to todo and removes the on-hold line. Only with the user's OK. |
| `drop ID "REASON"` | Moves a spec from todo, working, hold or done to dropped. It writes `> **Dropped (YYYY-MM-DD):** REASON` under the title. |
| `list [--json]`, `check` | Read-only. |

IDs can be given as `7` or `007`.

## The workflow

1. **Start of a session.** Run `list`. If a spec is in `working`, resume it: read its spec.md and carry on.
2. **Picking work.** Take the lowest id in `todo` unless the user says otherwise. Specs in `hold` are not
   picked. They wait for the user's OK, and then `resume` puts them back in todo.
3. **New work the user asks for, and follow-ups you find while working.** Each becomes a new spec via `init`.
   Fill in Goal, Background, Approach, Verification and Out of scope before you start. That is where the
   thinking happens, and it lets the user correct the plan cheaply. For an idea nobody will work on soon, a
   file in `specs/ideas/` is enough. When an idea is picked up, `init` it and delete the idea file.
4. **Doing the work.** Run `working ID`. Only one spec is in working at a time, so it is always clear what is
   in progress and work doesn't pile up half-finished.
5. **Finishing.** Run `done` only after the Verification passed and the Result section is written: what was
   built, the numbers, the keep/drop decision and any follow-ups. A spec in done without a Result tells the
   next reader nothing.
6. **When it didn't pay off.** If nothing was kept, or the premise turned out wrong, use `drop` with a
   one-line REASON a reader sees first: what was found, and why it isn't worth doing. Put the details in
   Result. A partial win (something kept) is `done`, with the dropped parts explained in Result. A dropped
   spec is never reopened; a new idea in the same area is a new spec.
7. **When it's blocked on the user.** Work the user parks, or that turns out to need something only the user
   can allow, goes to `hold` with the reason. Examples: a sudo or playbook run, a reboot, an install, spending
   money, a security trade-off.
8. **Work done without a spec.** Record it after the fact: `init`, `working`, `done`, then fill in spec.md.
   This keeps ids unique and increasing and the history complete.
9. **Committing.** Name the spec in the commit message (`spec 007: ...`) and run `check` before committing
   spec changes.

## Diagrams

- Use mermaid for flows and sequences: inline ```` ```mermaid ```` blocks in spec.md, or `.mmd` files.
- Use graphviz `.dot` files for dataflow and architecture graphs.
- Put diagram files in the spec folder and reference them from spec.md.
- Check that a `.dot` file renders (`dot -Tsvg file.dot > /dev/null`) before committing.

## Adopting in another repo

The script finds `<git root>/specs` by itself, so another repo only needs the folders and the rules:

1. Copy this skill folder to the repo's `.claude/skills/spec-driven/`.
2. Create `specs/{todo,working,hold,done,dropped}/.gitkeep` and `specs/ideas/README.md`.
3. Add a "Spec-driven development" section to the repo's AGENTS.md. Say what counts as non-trivial work there
   and what its verification looks like. Add `uv run .claude/skills/spec-driven/scripts/spec-manager.py list`
   to its "start here" steps.
4. Run `check`.

## Script notes

- **Dependencies:** it is a uv single-file script (Python 3.12+, pydantic and pyyaml); `uv run` installs them.
- **Logs:** they go to stderr as `YYYY-MM-DD HH:MM LEVEL message key=value`. stdout carries only the result:
  a path, a table or JSON.
- **Exit codes:**
  - `0`: success.
  - `1`: the tree is invalid or the spec isn't where the command needs it. A FAIL line says why, and nothing
    was changed.
  - `2`: a bad command line.
- **Location:** `--root DIR` points it at another spec tree, for example to check another repo's specs
  read-only.
