---
id: 064
name: wave-issues
description: Group issues into wave issues that form a graph, so parallel work starts safely and in the right order
---

# 064 wave-issues

**Depends on:** 063.

## Goal
Make it possible to ask what the next wave is, start every issue in a wave in parallel worktrees, and have an agent wait
until the wave is allowed to start. All 27 todo specs end up in wave issues, and the graph can be viewed and drawn.

## Background
Many todo specs could run at once, but dependencies alone don't say which are safe together. `routes.rs`, `server.rs`,
`migrate.rs`, `main.rs` and `workers.rs` were each touched by all of specs 018, 019 and 020, so parallel backend work can conflict.

The user first asked for YAML files in `./waves`, then replaced that idea in the same message: a wave is a normal GitHub issue,
like a Jira epic over ordinary issues, labelled `wave` with a title starting `wave: `. It references the issues that can run in
parallel and the waves that must finish first, and may form a graph. So nothing is added under `./waves`. The user decided that the 10
hold specs (052-058, 060-062) stay out of waves: no spec depends on them and no agent can do them. `next` lists them separately.

## Approach

**Wave issue body:**

```markdown
<description>

## Issues
- #23
- #37

## After
- #65
```

GitHub renders each `- #N` with its title and state, and shows the wave on each member's timeline. `## After` may list several parents.

**Wave status is computed from members**, never from the wave's open/closed flag, so a stale flag can't unblock anything:

| Status | Meaning |
|---|---|
| blocked | a parent wave isn't done |
| ready | parents done, no member started |
| active | parents done, some members working or closed |
| held | parents done, an open member is on hold (children stay blocked) |
| done | every member closed (done or dropped; a dropped member logs a warning) |

**`scripts/wave-manager.py`:** `next`, `list`, `show`, `gate` (exit 0 to go, 3 if blocked, `--wait` to poll), `dot`, `create`, `edit`, `sync`
and `check`. `check` covers cycles, bad parents, a member in two waves, empty waves, malformed bodies, and each spec's `Depends on`
being met by done work, its wave's ancestors or its own wave.

**`scripts/wave-plan.py`:** `graph`, `layers`, `propose`, `validate` and `apply`. `propose` drafts waves from dependencies and a file of code areas
per spec. `apply` creates a reviewed plan's waves through `wave-manager.py create`.

**`gh-spec-manager.py`:** the skeleton gets a `**Depends on:** none.` line, and `init --issue` refuses a wave issue.

**Real waves.** `areas.yaml` records code areas per spec, and `waves-plan.yaml` is the reviewed plan. Backend and console are separate tracks, since
they touch different files. Specs share a wave only where overlap should be limited to registration lists like routes and migrations.
Acceptance runs alone on a quiet machine, because other work skews its measurements.

**Docs.** `SKILL.md` becomes wave-aware, with picking work starting from `wave-manager.py next`. `AGENTS.md` describes waves.

## Verification
- Fake-`gh` tests pass for every status, cycle and malformed-body case, and the `gate` exit codes including `--wait` opening and timing out.
- The 27 `gh-spec-manager` tests still pass.
- Live: `validate` and `check` pass, `next` returns only the first backend wave plus the hold specs, `gate` is 0 for that wave and 3 for its child, and `show`
  is right on one wave of each status that exists.
- Every todo spec is in exactly one wave and every hold spec in none.
- `dot | dot -Tsvg` renders, and the image is viewed to judge layout and colors.

## Out of scope
- GitHub's native sub-issues, dependency links or Projects boards.
- Starting worktrees from the script. The skill says how an agent does it.
- Measuring merge conflicts. Grouping is judged from file history and spec text.

## Result
(Filled in when the spec moves to done or dropped: what was built, decisions, numbers, follow-ups.)
