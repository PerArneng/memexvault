---
name: gh-spec-driven
description: Spec-driven development where every spec is a GitHub issue and its content lives in a flat specs/ folder, managed by the bundled gh-spec-manager.py. The issue number is the spec's id and the issue holds its status (todo, working, hold, done, dropped). Wave issues group specs that are safe to do in parallel and form a graph, managed by wave-manager.py. Use this whenever starting any non-trivial piece of work (a feature, tool, experiment, design or technology decision, or a change to behavior), when the user says "make a spec", "add this to the queue", "what's next", "what's the next wave", "start wave N", "show the waves", "draw the waves", "pick up the next task", "park this", "drop that idea", when finishing or abandoning work, when recording work that was done without a spec, or when adopting spec-driven development in a repo, even if the user doesn't say "spec" or "issue".
---

# Spec-driven development with GitHub issues

Every non-trivial piece of work starts as a spec: a GitHub issue plus a short `spec.md` that says what will change, why,
how, and how to tell whether it worked.

GitHub holds the state, and the repo holds the content:
- **The issue number is the id.** GitHub assigns it, so two branches can't take the same one.
- **The issue holds the status.** It's the same from every branch and worktree.
- **`specs/<id>-<name>/` holds the content.** The folder never moves, and `spec.md` says nothing about status.

Dropped specs stay in the tree, and their issues keep the reason, so nobody tries the same dead end twice.

The bundled script is the only thing that creates specs or changes their status. It needs `gh` logged in with write
access to the repo. Never create spec folders, or change a spec issue's labels or open/closed state, by hand.

```bash
SM=".claude/skills/gh-spec-driven/scripts/gh-spec-manager.py"   # run from anywhere in the repo; it finds <git root>/specs
uv run $SM list              # every spec with its status, read from GitHub
uv run $SM check             # validate the tree and compare it with GitHub; run before committing spec changes
uv run $SM check --offline   # validate the tree only

WM=".claude/skills/gh-spec-driven/scripts/wave-manager.py"
uv run $WM next              # the waves that can start now, and what to start in them
```

## Layout

```
specs/
  NNN-kebab-name/        # NNN = the issue number, zero padded to at least 3 digits, e.g. 063-github-issue-specs
    spec.md              # the spec (required)
    *.mmd, *.dot, ...    # optional supplementary files (diagrams, data, notes, tools)
```

Ids can have gaps, because pull requests and wave issues share GitHub's number sequence. Wave issues have no folder.

## Status

| Status | GitHub issue |
|---|---|
| todo | open, no status label |
| working | open, `status:working` |
| hold | open, `status:hold`, reason in a comment |
| done | closed as completed |
| dropped | closed as not planned, reason in a comment |

Every spec issue has the label `spec`. Its title is `<name>: <description>` and its body links to the spec. Keep the content
in `spec.md`, not the issue, but discussion on the issue is fine. An issue without the `spec` label isn't a spec.

## spec.md

`spec.md` starts with a YAML header with exactly these keys, matching the folder name:

```
---
id: 063
name: github-issue-specs
description: One line saying what the spec achieves
---
```

Under the title comes a line naming the specs that must finish first, which the wave scripts read. Only its first sentence counts:

```
**Depends on:** 013, 017. Design: `docs/architecture.md`, section Search.
```

Ids, ranges and `except` work: `all of 002-030 except the dropped 010, plus 032.` Use `none.` if there are none.

It is followed by these sections. `init` writes them as a skeleton with hints:

| Section | What goes in it |
|---|---|
| **Goal** | What changes, and why it matters. Give numbers where they exist. |
| **Background** | Context, measurements, user requests, links to docs and earlier specs. |
| **Approach** | Files, components and steps. |
| **Verification** | The tests and the concrete check that decide keep or drop. |
| **Out of scope** | What the spec deliberately leaves out. |
| **Result** | Filled in at the end: what was built, numbers, decisions, follow-ups. |

## Waves

A wave is a set of issues that are fairly safe to do in parallel without many merge conflicts, like a Jira epic over ordinary issues.
It can be one issue, if that's safest. A wave is a normal GitHub issue with the label `wave`, a title starting `wave: `, and this body:

```markdown
What this wave does.

## Issues
- #23
- #37

## After
- #65
```

`## After` lists the waves that must finish first. A wave can have several parents and several children, so waves form a graph with
parallel branches. A wave's status is computed from its members, never from its own open or closed flag:

| Status | Meaning |
|---|---|
| blocked | a parent wave isn't done |
| ready | parents done, no member started |
| active | parents done, some members working or closed |
| held | parents done, an open member is on hold, and its children stay blocked |
| done | every member is closed (done or dropped) |

| Command (`wave-manager.py`) | Effect |
|---|---|
| `next [--json]` | The waves that can start now, what to start in them, and hold specs waiting on the user. |
| `list [--json]`, `show ID [--json]` | Every wave with its status, or one wave in detail. |
| `gate ID` | Exit 0 and print the issues to start if its parents are done. Exit 3 if not. |
| `gate ID --wait [--interval S] [--timeout S]` | Poll until it opens, by default every 300 s and with no time limit. Run it in the background. |
| `dot [--open-only] [--deps]` | A Graphviz diagram: `... dot \| dot -Tsvg -o waves.svg`. `--deps` adds spec dependency edges. |
| `create NAME "DESC" --issues .. [--after ..]` | Create a wave. |
| `edit ID [--add-issue ..] [--remove-issue ..] [--add-after ..] [--remove-after ..] [--description ..]` | Change a wave. Refused if it adds a problem to the graph. |
| `sync` | Close waves whose members are all closed, and reopen closed waves with open members. Run it after `done` or `drop`. |
| `check` | Validate the graph, including that every spec's `Depends on` is met by closed issues, earlier waves or its own wave. |

Create and edit waves only with these scripts. Hand-edited bodies must keep the exact `- #123` format, or the wave is skipped as malformed and
its children stay blocked.

**Starting a wave.** When asked to start wave N:
1. Run `gate N`. On exit 3, tell the user what it's waiting for, and run `gate N --wait` in the background if asked to wait. Don't start anything early.
2. Gate prints the issues to start. For each one, run `gh-spec-manager.py working ID` and work on it in its own worktree, with one agent per issue. A
   member on hold needs the user, not an agent.
3. Land the issues one at a time: merge the latest `main` into the branch, run its Verification, then merge to `main`, with the user's OK. Each
   branch resolves its own conflicts against what has already landed.
4. After each `done`, run `sync`. When all members are closed, the child waves open.

**Hold specs stay out of waves** unless something depends on them, because no agent can work on them. `next` lists them separately. A spec that goes on hold
while in a wave makes the wave `held`, so its children wait for the user's decision.

**Planning waves.** `wave-plan.py` helps group open specs:
- `graph [--json|--dot]` shows open specs and their dependencies, and `layers` shows topological levels.
- `propose --areas FILE [--exclude N ..]` drafts waves. In the areas file, each spec has a track and code areas. Specs sharing an area split into
  different waves, and different tracks become parallel waves.
- `validate PLAN [--areas FILE]` checks a plan. `apply PLAN [--dry-run]` creates its waves in graph order.

A draft is only a start, so read it critically. Dependency lines can be incomplete, and registration lists like routes and migrations overlap almost
everywhere. `specs/064-wave-issues/` has a worked example of an areas file and a plan. Parallel waves shouldn't share code areas, since nothing
stops agents working on both at once.

## Commands

The commands below are `gh-spec-manager.py`. Every command first validates the local tree. If any spec is malformed, it changes nothing and prints every problem at
once. `--dry-run` on `init`, `working`, `done`, `hold`, `resume` and `drop` shows what would happen.

| Command | Effect |
|---|---|
| `init NAME "DESCRIPTION"` | Creates the issue, then `specs/NNN-NAME/spec.md`, and prints its path. Fill in every section before starting. |
| `init NAME "DESCRIPTION" --issue N` | Same, but turns existing open issue N into the spec. Its body is kept, and the title is replaced. |
| `working ID` | todo to working. Several specs may be in working, for example in parallel worktrees. |
| `hold ID "REASON"` | todo or working to hold, with the reason as a comment. |
| `resume ID` | hold to todo. Only with the user's OK. |
| `done ID` | working to done. Refused unless `origin/main` has the spec with a filled-in Result. |
| `drop ID "REASON"` | todo, working, hold or done to dropped, with the reason as a comment. |
| `list [--json]`, `check [--offline]` | Read-only. |

IDs can be given as `7` or `007`.

## The workflow

1. **Start of a session.** Run `wave-manager.py next`, and `list` for all specs. If a spec is in `working`, check whether another session or worktree owns it before
   resuming: read its spec.md and its issue, and ask if it's unclear. A spec in working may have its folder and changes
   only on a branch. `list` marks such a spec "no folder on this branch".
2. **Picking work.** Take an issue that `wave-manager.py next` says to start, lowest id first, unless the user says otherwise. Don't start
   an issue from a blocked wave. Specs in `hold` are not picked. They wait for the user's OK, and then `resume` puts them back in todo.
3. **New work the user asks for, and follow-ups you find while working.** Each becomes a new spec via `init`. Fill in Goal,
   Background, Approach, Verification and Out of scope before you start. That is where the thinking happens, and it lets the
   user correct the plan cheaply. For an idea nobody will work on soon, an issue with the label `idea` and no folder is enough
   (`gh issue create --label idea`). When it's picked up, `init ... --issue N` makes it a spec. Commit a new spec's folder
   soon, since its issue is public and links to it. Fill in its `Depends on` line, and put it in a wave: add it to one that hasn't started if it's safe
   alongside those issues, or create a new wave with the right parents. `check` warns about open specs in no wave.
4. **Doing the work.** Run `working ID`.
5. **Finishing.** Write the Result after the Verification passed: what was built, the numbers, the keep/drop decision and any
   follow-ups. A spec in done without a Result tells the next reader nothing. Commit, merge and push with the user's
   OK, then run `done`, which refuses until `origin/main` has the Result. An issue only closes once its work has landed. Then run
   `wave-manager.py sync`.
6. **When it didn't pay off.** If nothing was kept, or the premise turned out wrong, use `drop` with a one-line REASON a reader
   sees first: what was found, and why it isn't worth doing. Put the details in Result. A partial win (something kept) is
   `done`, with the dropped parts explained in Result. A dropped spec is never reopened; a new idea in the same area is a new
   spec.
7. **When it's blocked on the user.** Work the user parks, or that turns out to need something only the user can allow, goes to
   `hold` with the reason. Examples: a sudo run, a reboot, an install, spending money, a security trade-off.
8. **Work done without a spec.** Record it after the fact: `init`, `working`, fill in spec.md, land it, `done`. This keeps the
   history complete.
9. **Committing.** Name the spec in the commit message (`spec #63: ...`), which links the commit to the issue, and run
   `check` before committing spec changes.

## Public repos

Issues on a public repo are public at once, and each new one may notify anyone watching it. Their numbers can't be reused.
Keep issue titles, bodies and comments to what could appear in the repo: no secrets, no private data, and nothing copied from
git-ignored material. Don't create test issues on the real repo; test against a fake `gh` on `PATH`, as
`specs/063-github-issue-specs/tools/test_gh_spec_manager.py` does.

## Diagrams

- Use mermaid for flows and sequences: inline ```` ```mermaid ```` blocks in spec.md, or `.mmd` files.
- Use graphviz `.dot` files for dataflow and architecture graphs.
- Put diagram files in the spec folder and reference them from spec.md.
- Check that a `.dot` file renders (`dot -Tsvg file.dot > /dev/null`) before committing.

## Adopting in another repo

The script finds `<git root>/specs` by itself and `gh` finds the repo from its git remote, so another repo needs:

1. This skill folder copied to the repo's `.claude/skills/gh-spec-driven/`, and a `specs/` folder (`mkdir specs`).
2. The labels: `spec`, `status:working`, `status:hold`, `idea` and `wave` (`gh label create NAME`).
3. A "Spec-driven development" section in the repo's AGENTS.md. Say what counts as non-trivial work there and what its
   verification looks like. Add `uv run .claude/skills/gh-spec-driven/scripts/wave-manager.py next` to its "start here" steps.
4. A passing `check`.

## Script notes

- **Dependencies:** each is a uv single-file script (Python 3.12+, with pydantic and sometimes pyyaml); `uv run` installs them. `wave-plan.py`
  loads `wave-manager.py` from its own folder. `gh` must be on
  `PATH` and logged in. `check --offline` doesn't use it.
- **Logs:** they go to stderr as `YYYY-MM-DD HH:MM LEVEL message key=value`. stdout carries only the result: a path, a URL, a
  table or JSON.
- **Exit codes:**
  - `0`: success.
  - `1`: the tree is invalid, the spec isn't in the status the command needs, or `gh` or `git` failed. A FAIL line says why.
  - `2`: a bad command line.
  - `3`: `wave-manager.py gate` only: the wave is blocked.
- **Partial failure:** GitHub calls aren't transactional. If a command fails midway, run `list` to see the status, then finish
  the rest with the same command or `gh`. If `init` fails after creating the issue, the FAIL line shows it; adopt that
  issue with `init NAME "DESCRIPTION" --issue N` after removing its `spec` label.
- **Location:** `--root DIR` points it at another flat spec folder.
