---
id: 063
name: github-issue-specs
description: Make every spec a GitHub issue, with status and ids from GitHub and content in a flat specs folder
---

# 063 github-issue-specs

## Goal
Every spec is a GitHub issue. The issue number is the spec's id, and the issue holds its status. `specs/` becomes one
flat folder of `specs/<id>-<name>/` folders that hold only content.

Outcome: 63 issues, one per spec, with number = id for all of them. Status is the same from any branch or worktree, two
branches can't take the same id, and the `spec-driven` skill is replaced by `gh-spec-driven`.

## Background
Status is currently the folder a spec sits in (`specs/{todo,working,hold,done,dropped}/`), so it differs per branch. While
specs 020, 027 and 034 ran in separate worktrees, `spec-manager.py list` on `main` showed all three as `todo`. Ids come
from the highest id on the current branch, so a branch without spec 061 would also have picked 061.

The user asked for this and chose: state and ids come from GitHub, the whole migration is done now, and several specs may
be in working at once. Before migration the repo is public with 0 issues and 0 PRs, so the first new issue is #1. All
spec text is already public on `origin/main`.

## Approach

**Ids and folders.** Issues are created in id order, so #N is spec N and every "spec 032" reference stays correct. Folder
names don't change: at least 3 digits, zero padded. Ids can have gaps from now on, because PRs share GitHub's number
sequence.

**Status lives only on the issue:**

| Status | GitHub issue |
|---|---|
| todo | open, no status label |
| working | open, `status:working` |
| hold | open, `status:hold`, reason in a comment |
| done | closed as completed |
| dropped | closed as not planned, reason in a comment |

**Issues are pointers.** The title is `<name>: <description>` and the body is the description plus a link to the spec on
`main`, with the `spec` label. The `> **On hold` and `> **Dropped` lines move out of `spec.md` into comments.

**Ideas.** `specs/ideas/` is removed. An idea is an issue labelled `idea` with no folder, and `init --issue N` promotes it,
or any other existing issue, to a spec.

**`done` guard.** `done` is refused unless `origin/main` has the spec with a filled-in Result, so an issue never closes
while its work sits only on a branch.

**Commit messages.** `spec #63: ...`, which links the commit to the issue. Old commits are left as they are.

**Steps:**
1. Build `.claude/skills/gh-spec-driven/` with `SKILL.md` and `scripts/gh-spec-manager.py`.
2. Test it against a fake `gh` in `tools/`. No real issue is created before step 3, since that would break id alignment.
3. Create the labels and issues with `tools/migrate.py`, stopping at once if a returned number isn't the expected id.
4. Flatten `specs/` with plain `mv`, so git-ignored spike data moves too. Strip the note lines and fix 3 old paths.
5. Delete `.claude/skills/spec-driven/` and update `AGENTS.md`.
6. Commit, push `main`, write the Result, push, and run `done 63`.

## Verification
- Fake-`gh` tests pass for every transition and bad case.
- `gh-spec-manager.py check` passes, and `list --json` matches a snapshot taken before migration for all 62 older specs on
  id, name, description and status.
- `gh issue list --state all --label spec` shows 63 issues with number = folder id. Spot-check one per status, including the
  close reason and comment.
- `specs/` holds only `NNN-name` folders, and git-ignored data has the same `du` sizes before and after.
- `git grep -E 'specs/(todo|working|hold|done|dropped)|spec-manager\.py|spec-driven/'` outside this spec finds only intended text.
- This spec goes through `working` and `done` live on #63. `hold`, `resume` and `drop` are covered by the fake-`gh` tests.

## Out of scope
- Moving spec content into issue bodies.
- GitHub Projects boards, milestones or dependency links between issues. `Depends on` lines stay in `spec.md`.
- Rewriting old commit messages.
- Changing other repos that use the `spec-driven` skill.

## Result
Done. Every spec is a GitHub issue with number = id, and `specs/` is flat. Kept as planned.

**Built:**
- `.claude/skills/gh-spec-driven/` replaces `.claude/skills/spec-driven/`, with `gh-spec-manager.py` (passes `mypy --strict`).
- `tools/migrate.py` (one-off), and `tools/test_gh_spec_manager.py` with `tools/fake_gh.py`.
- `AGENTS.md` updated.

**Measured:**
- Migration created 63 issues, with 63 of 63 returning the expected number, and applied 36 status changes with no failures.
- `gh issue list --state all --label spec` gives 63 issues, all with number and name matching their folder.
- `list --json` matches the pre-migration snapshot for all 62 older specs on id, name, description, status and folder. Status
  counts after migration: 27 todo, 24 done, 10 hold, 1 dropped, 1 working (this spec).
- Spot checks: #1 and #20 closed as completed, #10 closed as not planned with its reason, #52 open with `status:hold` and its
  reason, #59 open with only `spec`, #63 open with `status:working`.
- Fake-`gh` tests: 27 of 27 pass, covering every transition and bad case.
- All 21 git-ignored data folders have the same `du -sk` size before and after the move.
- The reference grep from Verification finds no old paths outside this spec. `check` passes.

**Live on the real repo:** `working 63` (the migration had set that label, so it was removed and set again with the command),
`working 63` refused when repeated, `done 63` refused while the Result was empty, then `done 63`.

**Not run live, by design:** `init`, `hold`, `resume` and `drop` have only run against the fake `gh`, to avoid test issues on the
public repo. `specs/035-web-workspace/tools/check_rules.sh` had a path fixed but wasn't rerun. `specs/` tools from older specs
weren't rerun after the move; a search found no relative paths in them that climb out of a spec folder.

**Decisions made along the way:** ids stay at 3 or more digits; `spec #N: ...` for new commits, with old ones unchanged; ideas become
issues labelled `idea`; `check` warns but doesn't fail on a spec issue with no folder on the branch, since it may be on another
branch.

**Follow-ups:** none needed. Other repos using the old `spec-driven` skill are unaffected.
