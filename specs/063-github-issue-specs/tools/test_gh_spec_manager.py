#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = []
# ///
"""Test gh-spec-manager.py end to end with a fake gh and a real temporary git repo with a bare origin.

Nothing here can reach GitHub: gh on PATH is fake_gh.py, and origin is a local folder. Pass the path to the
manager with --manager; by default it is found from the git root.

Run: uv run specs/063-github-issue-specs/tools/test_gh_spec_manager.py
"""

from __future__ import annotations

import json
import os
import shutil
import stat
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = Path(subprocess.run(["git", "rev-parse", "--show-toplevel"], cwd=HERE, capture_output=True, text=True, check=True).stdout.strip())
MANAGER = ROOT / ".claude/skills/gh-spec-driven/scripts/gh-spec-manager.py"
SPEC, WORKING, HOLD = "spec", "status:working", "status:hold"


def spec_md(n: int, name: str, result: str = "(Filled in when the spec moves to done or dropped: x)") -> str:
    return f"---\nid: {n:03d}\nname: {name}\ndescription: Desc of {name}\n---\n\n# {n:03d} {name}\n\n## Goal\nx\n\n## Result\n{result}\n"


class Base(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.tmp)
        self.repo = self.tmp / "repo"
        self.specs = self.repo / "specs"
        self.specs.mkdir(parents=True)
        self.bin = self.tmp / "bin"
        self.bin.mkdir()
        gh = self.bin / "gh"
        gh.write_text(f"#!/bin/sh\nexec {sys.executable} {HERE / 'fake_gh.py'} \"$@\"\n")
        gh.chmod(gh.stat().st_mode | stat.S_IEXEC)
        self.state = self.tmp / "state.json"
        self.state.write_text(json.dumps({"next": 1, "issues": {}, "calls": []}))
        self.env = {**os.environ, "PATH": f"{self.bin}:{os.environ['PATH']}", "FAKE_GH_STATE": str(self.state), "NO_COLOR": "1"}
        self.git("init", "-q", "-b", "main")
        self.git("config", "user.email", "t@example.invalid")
        self.git("config", "user.name", "t")
        origin = self.tmp / "origin.git"
        subprocess.run(["git", "init", "-q", "--bare", "-b", "main", str(origin)], check=True)
        self.git("remote", "add", "origin", str(origin))

    def git(self, *args: str) -> None:
        subprocess.run(["git", *args], cwd=self.repo, check=True, capture_output=True)

    def run_manager(self, *args: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(["uv", "run", "--quiet", str(MANAGER), "--root", str(self.specs), *args], cwd=self.repo,
                              env=self.env, capture_output=True, text=True, check=False)

    def ok(self, *args: str) -> str:
        r = self.run_manager(*args)
        self.assertEqual(r.returncode, 0, r.stderr)
        return r.stdout

    def refused(self, *args: str, contains: str) -> None:
        """The command exits 1 with a FAIL line, and GitHub writes and the folder stay as they were."""
        before = self.load()["issues"]
        tree = sorted(p.name for p in self.specs.iterdir())
        r = self.run_manager(*args)
        self.assertEqual(r.returncode, 1, f"stdout={r.stdout} stderr={r.stderr}")
        self.assertIn("FAIL", r.stderr)
        self.assertIn(contains, r.stderr)
        self.assertEqual(self.load()["issues"], before)
        self.assertEqual(sorted(p.name for p in self.specs.iterdir()), tree)

    def load(self) -> dict:
        return json.loads(self.state.read_text())

    def patch_state(self, **changes: object) -> None:
        self.state.write_text(json.dumps({**self.load(), **changes}))

    def issue(self, n: int) -> dict:
        return self.load()["issues"][str(n)]

    def status(self, n: int) -> str:
        rows = {int(r["id"]): r for r in json.loads(self.ok("list", "--json"))}
        return rows[n]["status"]

    def new_spec(self, name: str = "alpha") -> int:
        path = Path(self.ok("init", name, f"Desc of {name}").strip())
        return int(path.parent.name.split("-")[0])

    def publish(self, n: int, name: str, result: str) -> None:
        (self.specs / f"{n:03d}-{name}" / "spec.md").write_text(spec_md(n, name, result))
        self.git("add", "-A")
        self.git("commit", "-qm", "x")
        self.git("push", "-q", "origin", "main")


class Init(Base):
    def test_creates_issue_and_folder_with_matching_id(self) -> None:
        self.patch_state(next=70)
        path = Path(self.ok("init", "alpha", "Does a thing").strip())
        self.assertEqual(path, self.specs.resolve() / "070-alpha" / "spec.md")
        self.assertIn("id: 070\nname: alpha\ndescription: Does a thing\n", path.read_text())
        i = self.issue(70)
        self.assertEqual(i["title"], "alpha: Does a thing")
        self.assertEqual(i["labels"], [SPEC])
        self.assertIn("https://github.com/o/r/blob/main/specs/070-alpha/spec.md", i["body"])
        self.assertEqual(self.status(70), "todo")

    def test_description_with_colon_passes_own_validation(self) -> None:
        self.ok("init", "alpha", "Does: a thing")
        self.ok("check")

    def test_adopts_open_issue(self) -> None:
        issues = {"5": {"number": 5, "title": "bug report", "body": "keep me", "state": "OPEN", "stateReason": "",
                        "url": "https://github.com/o/r/issues/5", "labels": ["idea"], "comments": []}}
        self.patch_state(issues=issues, next=6)
        path = Path(self.ok("init", "fix-it", "Fix it", "--issue", "5").strip())
        self.assertEqual(path.parent.name, "005-fix-it")
        i = self.issue(5)
        self.assertEqual((i["title"], i["body"], i["labels"]), ("fix-it: Fix it", "keep me", [SPEC]))
        self.assertIn("specs/005-fix-it/spec.md", i["comments"][0])

    def test_adopt_refusals(self) -> None:
        self.refused("init", "x", "D", "--issue", "9", contains="no such issue")
        n = self.new_spec()
        self.refused("init", "y", "D", "--issue", str(n), contains="already a spec")
        issues = self.load()["issues"]
        issues["9"] = {"number": 9, "title": "t", "body": "", "state": "CLOSED", "stateReason": "COMPLETED", "url": "u", "labels": [], "comments": []}
        self.patch_state(issues=issues)
        self.refused("init", "x", "D", "--issue", "9", contains="only an open issue")

    def test_bad_arguments_refused(self) -> None:
        self.refused("init", "Bad_Name", "D", contains="kebab-case")
        self.refused("init", "ok-name", "  ", contains="one non-empty line")

    def test_name_taken_by_folder_or_by_issue_on_another_branch(self) -> None:
        self.new_spec("alpha")
        self.refused("init", "alpha", "D", contains="folder with this name")
        issues = self.load()["issues"]
        issues["40"] = {"number": 40, "title": "beta: on a branch", "body": "", "state": "OPEN", "stateReason": "", "url": "u", "labels": [SPEC], "comments": []}
        self.patch_state(issues=issues)
        self.refused("init", "beta", "D", contains="issue with this name")

    def test_dry_run_changes_nothing(self) -> None:
        self.ok("init", "alpha", "D", "--dry-run")
        self.assertEqual(self.load()["issues"], {})
        self.assertEqual(list(self.specs.iterdir()), [])

    def test_read_only_account_refused(self) -> None:
        self.patch_state(permission="READ")
        self.refused("init", "alpha", "D", contains="can't write")


class Transitions(Base):
    def test_working_hold_resume(self) -> None:
        n = self.new_spec()
        self.ok("working", str(n))
        self.assertEqual(self.status(n), "working")
        self.ok("hold", f"{n:03d}", "Needs a reboot")
        self.assertEqual(self.status(n), "hold")
        self.assertEqual(self.issue(n)["labels"], [SPEC, HOLD])
        self.assertRegex(self.issue(n)["comments"][-1], r"^\*\*On hold \(\d{4}-\d\d-\d\d\):\*\* Needs a reboot$")
        self.ok("resume", str(n))
        self.assertEqual(self.status(n), "todo")
        self.assertEqual(self.issue(n)["labels"], [SPEC])

    def test_hold_from_todo(self) -> None:
        n = self.new_spec()
        self.ok("hold", str(n), "Wait")
        self.assertEqual(self.status(n), "hold")

    def test_several_may_be_working(self) -> None:
        a, b = self.new_spec("alpha"), self.new_spec("beta")
        self.ok("working", str(a))
        self.ok("working", str(b))
        self.assertEqual((self.status(a), self.status(b)), ("working", "working"))

    def test_illegal_moves_refused(self) -> None:
        n = self.new_spec()
        self.refused("done", str(n), contains="needs a spec in working")
        self.refused("resume", str(n), contains="needs a spec in hold")
        self.ok("hold", str(n), "Wait")
        self.refused("working", str(n), contains="resume")
        self.refused("hold", str(n), "Again", contains="needs a spec in todo/working")

    def test_reason_required(self) -> None:
        n = self.new_spec()
        self.refused("hold", str(n), " ", contains="one non-empty line")
        self.refused("drop", str(n), "two\nlines", contains="one non-empty line")

    def test_unknown_and_invalid_ids(self) -> None:
        self.refused("working", "999", contains="no such issue")
        self.refused("working", "abc", contains="positive number")
        self.refused("working", "0", contains="positive number")

    def test_issue_without_spec_label_refused(self) -> None:
        issues = {"5": {"number": 5, "title": "t", "body": "", "state": "OPEN", "stateReason": "", "url": "u", "labels": [], "comments": []}}
        self.patch_state(issues=issues, next=6)
        self.refused("working", "5", contains="not a spec")

    def test_gh_failure_is_reported(self) -> None:
        n = self.new_spec()
        self.patch_state(fail_on="issue edit")
        self.refused("working", str(n), contains="HTTP 502")

    def test_dry_run_changes_nothing(self) -> None:
        n = self.new_spec()
        before = self.issue(n)
        for args in (("working", str(n)), ("hold", str(n), "R"), ("drop", str(n), "R")):
            self.ok(*args, "--dry-run")
        self.assertEqual(self.issue(n), before)


class Done(Base):
    def working_spec(self) -> int:
        n = self.new_spec()
        self.ok("working", str(n))
        return n

    def test_refused_until_on_origin_main_with_result(self) -> None:
        n = self.working_spec()
        self.git("commit", "-q", "--allow-empty", "-m", "base")
        self.git("push", "-q", "origin", "main")
        self.refused("done", str(n), contains="doesn't have this spec")
        self.publish(n, "alpha", "(Filled in when the spec moves to done or dropped: x)")
        self.refused("done", str(n), contains="no filled-in Result")
        self.publish(n, "alpha", "")
        self.refused("done", str(n), contains="no filled-in Result")

    def test_local_result_alone_is_not_enough(self) -> None:
        n = self.working_spec()
        self.publish(n, "alpha", "(Filled in when the spec moves to done or dropped: x)")
        (self.specs / f"{n:03d}-alpha" / "spec.md").write_text(spec_md(n, "alpha", "Built it, not pushed"))
        self.refused("done", str(n), contains="no filled-in Result")

    def test_closes_as_completed(self) -> None:
        n = self.working_spec()
        self.publish(n, "alpha", "Built it")
        self.ok("done", str(n))
        i = self.issue(n)
        self.assertEqual((i["state"], i["stateReason"], i["labels"]), ("CLOSED", "COMPLETED", [SPEC]))
        self.assertEqual(self.status(n), "done")

    def test_needs_folder_on_this_branch(self) -> None:
        n = self.working_spec()
        shutil.rmtree(self.specs / f"{n:03d}-alpha")
        self.refused("done", str(n), contains="no spec folder")


class Drop(Base):
    def test_from_each_status(self) -> None:
        for name, setup in (("todo-spec", ()), ("work-spec", ("working",)), ("hold-spec", ("hold", "Wait"))):
            n = self.new_spec(name)
            if setup:
                self.ok(setup[0], str(n), *setup[1:])
            self.ok("drop", str(n), "Not worth it")
            i = self.issue(n)
            self.assertEqual((i["state"], i["stateReason"], i["labels"]), ("CLOSED", "NOT_PLANNED", [SPEC]))
            self.assertRegex(i["comments"][-1], r"^\*\*Dropped \(\d{4}-\d\d-\d\d\):\*\* Not worth it$")
            self.assertEqual(self.status(n), "dropped")

    def test_from_done_and_never_twice(self) -> None:
        n = self.new_spec()
        self.ok("working", str(n))
        self.publish(n, "alpha", "Built it")
        self.ok("done", str(n))
        self.ok("drop", str(n), "Refiled")
        self.assertEqual(self.status(n), "dropped")
        self.assertIn(["issue", "reopen", str(n)], self.load()["calls"])
        self.refused("drop", str(n), "Again", contains="needs a spec in")
        self.refused("working", str(n), contains="needs a spec in todo")


class ListAndCheck(Base):
    def write(self, folder: str, text: str) -> None:
        (self.specs / folder).mkdir()
        (self.specs / folder / "spec.md").write_text(text)

    def add_issue(self, n: int, title: str, labels: list[str], state: str = "OPEN", reason: str = "") -> None:
        issues = self.load()["issues"]
        issues[str(n)] = {"number": n, "title": title, "body": "", "state": state, "stateReason": reason,
                          "url": f"https://github.com/o/r/issues/{n}", "labels": labels, "comments": []}
        self.patch_state(issues=issues)

    def test_every_status_and_a_branch_only_issue(self) -> None:
        cases = [("todo", [SPEC], "OPEN", ""), ("working", [SPEC, WORKING], "OPEN", ""), ("hold", [SPEC, HOLD], "OPEN", ""),
                 ("done", [SPEC], "CLOSED", "COMPLETED"), ("dropped", [SPEC], "CLOSED", "NOT_PLANNED")]
        for n, (status, labels, state, reason) in enumerate(cases, start=1):
            self.write(f"{n:03d}-s{n}", spec_md(n, f"s{n}"))
            self.add_issue(n, f"s{n}: Desc of s{n}", labels, state, reason)
        self.add_issue(9, "on-branch: Elsewhere", [SPEC])
        rows = json.loads(self.ok("list", "--json"))
        self.assertEqual([r["status"] for r in rows], [c[0] for c in cases] + ["todo"])
        self.assertEqual((rows[-1]["id"], rows[-1]["name"], rows[-1]["description"], rows[-1]["folder"]), ("009", "on-branch", "Elsewhere", None))
        self.assertIn("(no folder on this branch)", self.ok("list"))
        r = self.run_manager("check")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("WARN spec issue has no folder", r.stderr)

    def test_folder_without_issue_listed_and_check_fails(self) -> None:
        self.write("001-a", spec_md(1, "a"))
        self.assertEqual(json.loads(self.ok("list", "--json"))[0]["status"], "no-issue")
        self.assertIn("no issue #1", self.run_manager("check").stderr)
        self.ok("check", "--offline")

    def test_state_contradictions_fail_check(self) -> None:
        self.write("001-a", spec_md(1, "a"))
        self.add_issue(1, "a: x", [SPEC, WORKING, HOLD])
        self.assertIn("has both", self.run_manager("check").stderr)
        self.add_issue(1, "a: x", [SPEC, HOLD], "CLOSED", "COMPLETED")
        self.assertIn("closed but still", self.run_manager("check").stderr)
        self.add_issue(1, "other: x", [SPEC])
        self.assertIn("title must start", self.run_manager("check").stderr)

    def test_tree_problems_all_reported_and_block_writes(self) -> None:
        self.write("001-a", "no header")
        self.write("002-b", spec_md(3, "b"))
        self.write("004-c", spec_md(4, "other"))
        self.write("bad_folder", spec_md(5, "x"))
        (self.specs / "006-nospec").mkdir()
        (self.specs / "stray.txt").write_text("x")
        self.write("007-d", spec_md(7, "d"))
        self.write("007-e", spec_md(7, "e"))
        self.write("008-f", spec_md(8, "f").replace("## Goal", "> **On hold (2026-01-01):** x\n\n## Goal"))
        r = self.run_manager("check", "--offline")
        self.assertEqual(r.returncode, 1)
        for text in ("must start with", "header id 003", "header name other", "folder name must be", "spec.md is missing",
                     "unexpected file", "duplicate id", "status lives on the issue"):
            self.assertIn(text, r.stderr)
        self.refused("init", "new", "D", contains="tree is invalid")


if __name__ == "__main__":
    unittest.main()
