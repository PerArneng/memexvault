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
Done. Wave issues #65-#80 cover all 27 todo specs, and the skill and `AGENTS.md` are wave-aware. Kept as planned.

**Built:**
- `.claude/skills/gh-spec-driven/scripts/wave-manager.py` and `wave-plan.py`, both passing `mypy --strict`.
- In `gh-spec-manager.py`, a `**Depends on:** none.` line in the skeleton, and `init --issue` refuses a wave issue.
- `areas.yaml`, `waves-plan.yaml`, `waves.svg` and `tools/test_waves.py` here. GitHub is the source of truth for waves now, so the plan is a record, not live data.

**The waves.** `waves.svg` shows the graph. It's a snapshot; `wave-manager.py dot` draws the current state.

| Issue | Wave | Issues | After |
|---|---|---|---|
| #65 | entities, browser session, Postgres TLS | 23, 37, 59 | none |
| #66 | wiki pages, extraction agent | 22, 24 | #65 |
| #67 | console shell | 40 | #65 |
| #68 | caches, review gate, MCP server | 21, 25, 29 | #66 |
| #69 | wiki screen | 44 | #67, #66 |
| #70 | audit agent, meeting ingest | 26, 28 | #68 |
| #71 | activity stream, chat backend | 38, 49 | #70 |
| #72 | document viewer screen | 42 | #67, #70 |
| #73 | backup, restore and erasure | 30 | #71 |
| #74 | ingest, search, entity and review screens | 41, 43, 45, 46 | #72, #71 |
| #75 | admin API | 39 | #73 |
| #76 | chat screen | 50 | #74 |
| #77 | audit and admin screens | 47, 48 | #75, #67 |
| #78 | backend acceptance | 31 | #75, #69, #76, #77 |
| #79 | 1M scale benchmark | 33 | #78 |
| #80 | console acceptance | 51 | #78 |

**Measured:**
- `test_waves.py`: 47 of 47 pass. The 27 `gh-spec-manager` tests still pass.
- Five deliberate bugs were put in `wave-manager.py`. The tests caught four. Ignoring `except` got through, so a test was added for it, but the five weren't rerun after that.
- Live: `validate` and `check` pass. `next` returns only #65 plus the 10 hold specs, and #64 as an open spec in no wave. `gate 65` exits 0 and prints 23, 37 and 59. `gate 66` exits 3,
  waiting for #65. `show 66` is right. All 27 todo specs are in exactly one wave, and no hold spec is in any.
- Only `ready` and `blocked` exist live so far. `active`, `held` and `done` are covered by fake-`gh` tests, as are `create` and `edit` beyond the 16 real creations, `sync` closing a wave, and `gate --wait`.
- First live runs of `init` and `working` worked, on this spec. `hold`, `resume` and `drop` still haven't run live.
- `dot | dot -Tsvg` renders with no warnings. The first layout stretched clusters across columns, so each wave's members now share a rank.

**Draft compared with the reviewed plan.** `propose` made 15 waves and validated, but the reviewed plan differs on purpose:
- The draft started #49 chat backend in the first wave. Its `AgentRunner` extension needs the trait from #24, which its `Depends on` line didn't say and `crates/` confirms doesn't exist yet. 024 was added to that line.
- The draft put #21 caches beside #23 entities in the first wave. Both change search, so #21 moved later.
- The draft ran backend acceptance as soon as the backend finished, with console work still running. The reviewed plan waits for the console waves, for a quiet machine.

**Accepted warnings from `validate --areas`:** #38 and #49 share `streams`, as separate routes and modules. Parallel waves #79 and #80 share `quiet-machine`, since making #80
wait for #79 would make console acceptance wait for a disk approval. If the benchmark runs at the same time, it could disturb #80.

**Limits:** the grouping is judgment from file history and spec text, not a measurement, and #65, #66 and others can still conflict on `routes.rs` and `migrate.rs`. Real wave runs will show whether it's right.

**Follow-ups:** none required. Waves can be changed with `wave-manager.py edit`.
