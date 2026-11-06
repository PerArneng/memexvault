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
(Filled in when the spec moves to done or dropped: what was built, decisions, numbers, follow-ups.)
