#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = []
# ///
"""Test wave-manager.py and wave-plan.py end to end against a fake gh. Nothing here can reach GitHub.

Run: uv run specs/064-wave-issues/tools/test_waves.py
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
SCRIPTS = ROOT / ".claude/skills/gh-spec-driven/scripts"
FAKE = ROOT / "specs/063-github-issue-specs/tools/fake_gh.py"
SPEC, WAVE, WORKING, HOLD = "spec", "wave", "status:working", "status:hold"


class Base(unittest.TestCase):
    def setUp(self) -> None:
        self.tmp = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.tmp)
        self.repo = self.tmp / "repo"
        self.specs = self.repo / "specs"
        self.specs.mkdir(parents=True)
        bin_dir = self.tmp / "bin"
        bin_dir.mkdir()
        gh = bin_dir / "gh"
        gh.write_text(f"#!/bin/sh\nexec {sys.executable} {FAKE} \"$@\"\n")
        gh.chmod(gh.stat().st_mode | stat.S_IEXEC)
        self.state = self.tmp / "state.json"
        self.state.write_text(json.dumps({"next": 200, "issues": {}, "calls": []}))
        self.env = {**os.environ, "PATH": f"{bin_dir}:{os.environ['PATH']}", "FAKE_GH_STATE": str(self.state), "NO_COLOR": "1"}

    def load(self) -> dict:
        return json.loads(self.state.read_text())

    def put(self, n: int, title: str, labels: list[str], state: str = "OPEN", reason: str = "", body: str = "") -> None:
        data = self.load()
        data["issues"][str(n)] = {"number": n, "title": title, "body": body, "state": state, "stateReason": reason,
                                  "url": f"https://github.com/o/r/issues/{n}", "labels": labels, "comments": []}
        self.state.write_text(json.dumps(data))

    def spec(self, n: int, depends: str | None = None, labels: tuple[str, ...] = (), state: str = "OPEN", reason: str = "") -> None:
        name = f"s{n}"
        self.put(n, f"{name}: Desc", [SPEC, *labels], state, reason)
        (self.specs / f"{n:03d}-{name}").mkdir()
        dep = f"**Depends on:** {depends}\n\n" if depends else ""
        (self.specs / f"{n:03d}-{name}" / "spec.md").write_text(
            f"---\nid: {n:03d}\nname: {name}\ndescription: Desc\n---\n\n# {n:03d} {name}\n\n{dep}## Goal\nx\n")

    def done(self, n: int, depends: str | None = None) -> None:
        self.spec(n, depends, state="CLOSED", reason="COMPLETED")

    def wave(self, n: int, issues: list[int], after: list[int] | None = None, name: str | None = None, state: str = "OPEN") -> None:
        body = "Does things\n\n## Issues\n" + "".join(f"- #{i}\n" for i in issues)
        if after:
            body += "\n## After\n" + "".join(f"- #{i}\n" for i in after)
        self.put(n, f"wave: {name or f'w{n}'}", [WAVE], state, body=body)

    def script(self, name: str, *args: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(["uv", "run", "--quiet", str(SCRIPTS / name), "--root", str(self.specs), *args], cwd=self.repo, env=self.env,
                              capture_output=True, text=True, check=False)

    def wm(self, *args: str) -> subprocess.CompletedProcess[str]:
        return self.script("wave-manager.py", *args)

    def ok(self, *args: str) -> str:
        r = self.wm(*args)
        self.assertEqual(r.returncode, 0, r.stderr)
        return r.stdout

    def problems(self, contains: str) -> None:
        r = self.wm("check")
        self.assertEqual(r.returncode, 1, r.stderr)
        self.assertIn(contains, r.stderr)

    def statuses(self) -> dict[int, str]:
        return {v["number"]: v["status"] for v in json.loads(self.ok("list", "--json"))}

    def refused(self, *args: str, contains: str) -> None:
        before = self.load()["issues"]
        r = self.wm(*args)
        self.assertEqual(r.returncode, 1, f"{r.stdout} {r.stderr}")
        self.assertIn(contains, r.stderr)
        self.assertEqual(self.load()["issues"], before)


class Status(Base):
    def test_every_status(self) -> None:
        self.done(1)
        self.done(2)
        self.spec(3)
        self.spec(4, labels=(WORKING,))
        self.spec(5)
        self.spec(6, labels=(HOLD,))
        self.spec(7)
        self.spec(8)
        self.wave(100, [1, 2])          # done
        self.wave(101, [3], [100])      # ready
        self.wave(102, [4, 5], [100])   # active
        self.wave(103, [6, 7], [100])   # held
        self.wave(104, [8], [101])      # blocked
        self.assertEqual(self.statuses(), {100: "done", 101: "ready", 102: "active", 103: "held", 104: "blocked"})

    def test_closed_member_alone_makes_a_wave_active(self) -> None:
        self.done(1)
        self.spec(2)
        self.wave(100, [1, 2])
        self.assertEqual(self.statuses(), {100: "active"})

    def test_dropped_member_counts_as_closed_with_a_warning(self) -> None:
        self.spec(1, state="CLOSED", reason="NOT_PLANNED")
        self.spec(2)
        self.wave(100, [1])
        self.wave(101, [2], [100])
        self.assertEqual(self.statuses(), {100: "done", 101: "ready"})
        self.assertIn("was dropped", self.wm("check").stderr)

    def test_status_ignores_the_wave_flag(self) -> None:
        self.spec(1)
        self.spec(2)
        self.wave(100, [1], state="CLOSED")   # closed by hand, though #1 is open
        self.wave(101, [2], [100])
        self.assertEqual(self.statuses()[101], "blocked")
        self.problems("is closed but has open issues")

    def test_several_parents_all_must_be_done(self) -> None:
        self.done(1)
        self.spec(2)
        self.spec(3)
        self.wave(100, [1])
        self.wave(101, [2])
        self.wave(102, [3], [100, 101])
        self.assertEqual(self.statuses()[102], "blocked")
        self.put(2, "s2: Desc", [SPEC], "CLOSED", "COMPLETED")
        self.assertEqual(self.statuses()[102], "ready")


class Next(Base):
    def test_returns_parallel_waves_and_hold_specs_apart(self) -> None:
        self.done(1)
        self.spec(2)
        self.spec(3)
        self.spec(4)
        self.spec(5, labels=(HOLD,))
        self.spec(6)
        self.wave(100, [1])
        self.wave(101, [2], [100])
        self.wave(102, [3], [100])
        self.wave(103, [4], [101])
        report = json.loads(self.ok("next", "--json"))
        self.assertEqual([w["number"] for w in report["waves"]], [101, 102])
        self.assertEqual([m["number"] for m in report["hold_outside_waves"]], [5])
        self.assertEqual([m["number"] for m in report["open_outside_waves"]], [6])
        text = self.ok("next")
        self.assertIn("start: #2 s2", text)
        self.assertIn("On hold, waiting on the user", text)

    def test_says_why_when_nothing_can_start(self) -> None:
        self.assertIn("no waves", self.ok("next"))
        self.done(1)
        self.wave(100, [1])
        self.assertIn("every wave is done", self.ok("next"))
        self.spec(2)
        self.spec(3)
        self.wave(101, [2], [102])
        self.wave(102, [3], [101])
        self.assertIn("suggests a cycle", self.ok("next"))


class Gate(Base):
    def graph(self) -> None:
        self.spec(1)
        self.spec(2)
        self.spec(3, labels=(WORKING,))
        self.wave(100, [1, 3])
        self.wave(101, [2], [100])

    def test_open_prints_only_what_to_start(self) -> None:
        self.graph()
        r = self.wm("gate", "100")
        self.assertEqual((r.returncode, r.stdout), (0, "1\n"))

    def test_blocked_exits_3_and_names_the_parent(self) -> None:
        self.graph()
        r = self.wm("gate", "101")
        self.assertEqual((r.returncode, r.stdout), (3, ""))
        self.assertIn("waiting_for=[100]", r.stderr)

    def test_unknown_wave_is_a_failure_not_blocked(self) -> None:
        self.graph()
        self.assertEqual(self.wm("gate", "1").returncode, 1)
        self.assertEqual(self.wm("gate", "abc").returncode, 1)

    def test_bad_parent_blocks(self) -> None:
        self.spec(1)
        self.wave(100, [1], [999])
        self.assertEqual(self.wm("gate", "100").returncode, 3)

    def test_wait_times_out_with_3(self) -> None:
        self.graph()
        r = self.wm("gate", "101", "--wait", "--interval", "0.05", "--timeout", "0.2")
        self.assertEqual(r.returncode, 3)
        self.assertIn("gave up waiting", r.stderr)

    def test_wait_opens_when_the_parent_finishes(self) -> None:
        self.graph()
        data = self.load()
        data["close_after_lists"] = {"lists": 3, "numbers": [1, 3]}
        self.state.write_text(json.dumps(data))
        r = self.wm("gate", "101", "--wait", "--interval", "0.05", "--timeout", "30")
        self.assertEqual((r.returncode, r.stdout), (0, "2\n"), r.stderr)

    def test_held_wave_may_start_and_warns(self) -> None:
        self.spec(1, labels=(HOLD,))
        self.spec(2)
        self.wave(100, [1, 2])
        r = self.wm("gate", "100")
        self.assertEqual((r.returncode, r.stdout), (0, "2\n"))
        self.assertIn("on hold", r.stderr)


class Structure(Base):
    def test_valid_graph_passes(self) -> None:
        self.done(1)
        self.spec(2, "001.")
        self.wave(100, [1])
        self.wave(101, [2], [100])
        self.ok("check")

    def test_cycle(self) -> None:
        self.spec(1)
        self.spec(2)
        self.spec(3)
        self.wave(100, [1], [102])
        self.wave(101, [2], [100])
        self.wave(102, [3], [101])
        self.problems("waves form a cycle")

    def test_self_parent(self) -> None:
        self.spec(1)
        self.wave(100, [1], [100])
        self.problems("lists itself")

    def test_bad_members_and_parents(self) -> None:
        self.spec(1)
        self.spec(2)
        self.wave(100, [1])
        cases = [(([1], None), "is also in wave"), (([999], None), "doesn't exist"), (([100], None), "is a wave"), (([2], [1]), "isn't a valid wave"),
                 (([2, 2], None), "lists an issue twice"), (([], None), "has no issues")]
        for (issues, after), text in cases:
            self.wave(101, issues, after)
            self.problems(text)

    def test_malformed_bodies_are_reported_and_block_children(self) -> None:
        self.spec(1)
        self.spec(2)
        for body, text in (("no sections", "section is missing"), ("x\n\n## Issues\n- 1\n", "must look like"), ("x\n\n## Other\n", "unknown section"),
                           ("x\n\n## Issues\n- #1\n## Issues\n- #1\n", "appears twice")):
            self.put(100, "wave: bad", [WAVE], body=body)
            self.wave(101, [2], [100])
            self.problems(text)
            self.assertEqual(self.statuses(), {101: "blocked"})

    def test_wrong_title(self) -> None:
        self.put(100, "not a wave title", [WAVE], body="x\n\n## Issues\n- #1\n")
        self.problems("title must start with")

    def test_crlf_body_parses(self) -> None:
        self.spec(1)
        self.put(100, "wave: web edit", [WAVE], body="Text\r\n\r\n## Issues\r\n- #1\r\n")
        self.assertEqual(self.statuses(), {100: "ready"})


class Dependencies(Base):
    def test_forms_of_the_depends_line(self) -> None:
        for n in range(1, 6):
            self.done(n)
        self.spec(10, "all of 001-005 except the dropped 003, plus 007. Design: 009.")
        self.spec(7)
        self.wave(100, [7])
        self.wave(101, [10], [100])
        self.ok("check")
        self.wave(101, [10])
        self.problems("depends on open #7 in wave #100")

    def test_excepted_and_later_sentence_ids_are_not_dependencies(self) -> None:
        self.spec(3)
        self.spec(9)
        self.spec(10, "all of 001-002 except the dropped 003. See 009.")
        self.done(1)
        self.done(2)
        self.wave(100, [10])
        self.wave(101, [3])
        self.wave(102, [9])
        self.ok("check")

    def test_except_removes_an_id_from_inside_a_range(self) -> None:
        self.done(1)
        self.spec(2)  # open, and in no earlier wave, so only the except clause makes this valid
        self.done(3)
        self.spec(10, "all of 001-003 except the dropped 002.")
        self.wave(100, [10])
        self.wave(101, [2])
        self.ok("check")

    def test_ancestor_counts_transitively(self) -> None:
        self.spec(1)
        self.spec(2)
        self.spec(3, "001.")
        self.wave(100, [1])
        self.wave(101, [2], [100])
        self.wave(102, [3], [101])
        self.ok("check")

    def test_parallel_wave_does_not_count(self) -> None:
        self.spec(1)
        self.spec(2, "001.")
        self.wave(100, [1])
        self.wave(101, [2])
        self.problems("depends on open #1 in wave #100")

    def test_open_dependency_in_no_wave_fails(self) -> None:
        self.spec(1)
        self.spec(2, "001.")
        self.wave(100, [2])
        self.problems("in no wave")

    def test_same_wave_warns(self) -> None:
        self.spec(1)
        self.spec(2, "001.")
        self.wave(100, [1, 2])
        r = self.wm("check")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("can't start together", r.stderr)

    def test_open_spec_in_no_wave_warns(self) -> None:
        self.spec(1)
        self.spec(2)
        self.wave(100, [1])
        r = self.wm("check")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("#2 s2 is in no wave", r.stderr)


class Changes(Base):
    def test_create_with_commas_and_hashes(self) -> None:
        self.spec(1)
        self.spec(2)
        self.wave(100, [1])
        url = self.ok("create", "my wave", "What it does", "--issues", "#2", "--after", "100").strip()
        self.assertEqual(url, "https://github.com/o/r/issues/200")
        i = self.load()["issues"]["200"]
        self.assertEqual((i["title"], i["labels"]), ("wave: my wave", [WAVE]))
        self.assertEqual(i["body"], "What it does\n\n## Issues\n- #2\n\n## After\n- #100\n")
        self.spec(3)
        self.spec(4)
        self.ok("create", "other", "D", "--issues", "3,4")
        self.assertEqual(self.load()["issues"]["201"]["body"], "D\n\n## Issues\n- #3\n- #4\n")

    def test_create_refusals(self) -> None:
        self.spec(1)
        self.spec(2, "001.")
        self.wave(100, [1])
        self.refused("create", "w100", "D", "--issues", "2", contains="title exists")
        self.refused("create", "x", "D", "--issues", "1", contains="also in wave")
        self.refused("create", "x", " ", "--issues", "2", contains="needs a description")
        self.refused("create", "x", "a\n## Issues", "--issues", "2", contains="headings")
        self.refused("create", "x", "D", "--issues", "2", "--after", "1", contains="isn't a valid wave")
        self.refused("create", "x", "D", "--issues", "2", contains="depends on open #1")
        self.refused("create", "x", "D", "--issues", "abc", contains="expected an issue number")

    def test_read_only_account_and_dry_run(self) -> None:
        self.spec(1)
        data = self.load()
        data["permission"] = "READ"
        self.state.write_text(json.dumps(data))
        self.refused("create", "x", "D", "--issues", "1", contains="can't write")
        self.ok("create", "x", "D", "--issues", "1", "--dry-run")
        self.assertEqual(sorted(self.load()["issues"]), ["1"])

    def test_edit(self) -> None:
        self.spec(1)
        self.spec(2)
        self.spec(3)
        self.wave(100, [1])
        self.wave(101, [2], [100])
        self.ok("edit", "101", "--add-issue", "3", "--remove-after", "100", "--description", "New")
        self.assertEqual(self.load()["issues"]["101"]["body"], "New\n\n## Issues\n- #2\n- #3\n")
        self.ok("edit", "101", "--remove-issue", "3", "--add-after", "100")
        self.assertEqual(self.load()["issues"]["101"]["body"], "New\n\n## Issues\n- #2\n\n## After\n- #100\n")

    def test_edit_refuses_what_breaks_the_graph(self) -> None:
        self.spec(1)
        self.spec(2, "001.")
        self.wave(100, [1])
        self.wave(101, [2], [100])
        self.refused("edit", "100", "--add-after", "101", contains="cycle")
        self.refused("edit", "101", "--remove-after", "100", contains="depends on open #1")
        self.refused("edit", "101", "--remove-issue", "9", contains="isn't there")
        self.refused("edit", "101", "--remove-issue", "2", contains="has no issues")

    def test_edit_can_fix_an_existing_problem(self) -> None:
        self.spec(1)
        self.spec(2)
        self.wave(100, [1], [999])
        self.wave(101, [2], [888])
        self.ok("edit", "100", "--remove-after", "999")
        self.assertEqual(self.statuses()[100], "ready")

    def test_sync_closes_and_reopens(self) -> None:
        self.done(1)
        self.spec(2)
        self.spec(3)
        self.wave(100, [1])
        self.wave(101, [2], state="CLOSED")
        self.wave(102, [3])
        self.ok("sync", "--dry-run")
        self.assertEqual(self.load()["issues"]["100"]["state"], "OPEN")
        self.ok("sync")
        issues = self.load()["issues"]
        self.assertEqual((issues["100"]["state"], issues["100"]["stateReason"]), ("CLOSED", "COMPLETED"))
        self.assertEqual((issues["101"]["state"], issues["102"]["state"]), ("OPEN", "OPEN"))

    def test_init_refuses_a_wave_and_writes_a_depends_line(self) -> None:
        self.spec(1)
        self.wave(100, [1])
        r = self.script("gh-spec-manager.py", "init", "x", "D", "--issue", "100")
        self.assertEqual(r.returncode, 1)
        self.assertIn("wave issue can't become a spec", r.stderr)
        r = self.script("gh-spec-manager.py", "init", "newspec", "D")
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("**Depends on:** none.", Path(r.stdout.strip()).read_text())


class Dot(Base):
    def test_renders_with_real_graphviz(self) -> None:
        self.done(1)
        self.spec(2, "001.", labels=(WORKING,))
        self.spec(3, "002.", labels=(HOLD,))
        self.spec(4, "002.")
        self.put(5, 'odd "title" <b>', [], "CLOSED", "NOT_PLANNED")
        self.wave(100, [1, 5], name='A & "B" <c>')
        self.wave(101, [2, 3], [100])
        self.wave(102, [4], [101])
        out = self.ok("dot", "--deps")
        self.assertIn("lhead=cluster_101", out)
        self.assertIn("style=dashed", out)
        self.assertIn("legend_todo", out)
        if shutil.which("dot"):
            r = subprocess.run(["dot", "-Tsvg"], input=out, capture_output=True, text=True, check=False)
            self.assertEqual(r.returncode, 0, r.stderr)
            self.assertIn("<svg", r.stdout)
            self.assertEqual(r.stderr, "")

    def test_open_only_hides_done_waves(self) -> None:
        self.done(1)
        self.spec(2)
        self.wave(100, [1])
        self.wave(101, [2], [100])
        out = self.ok("dot", "--open-only")
        self.assertNotIn("cluster_100", out)
        self.assertIn("cluster_101", out)


class Plan(Base):
    def wp(self, *args: str) -> subprocess.CompletedProcess[str]:
        return self.script("wave-plan.py", *args)

    def write(self, name: str, text: str) -> str:
        path = self.tmp / name
        path.write_text(text)
        return str(path)

    def chain(self) -> None:
        self.done(1)
        self.spec(2, "001.")
        self.spec(3, "001.")
        self.spec(4, "002.")
        self.spec(5, labels=(HOLD,))
        self.spec(6, "002.")

    PLAN = """
waves:
  - {key: A, name: first, description: D1, issues: [2, 3]}
  - {key: B, name: second, description: D2, issues: [4], after: [A]}
  - {key: C, name: third, description: D3, issues: [6], after: [A]}
"""

    def test_graph_and_layers(self) -> None:
        self.chain()
        nodes = {n["number"]: n for n in json.loads(self.wp("graph", "--json").stdout)}
        self.assertEqual((nodes[2]["depends"], nodes[4]["depends"], nodes[5]["status"]), ([], [2], "hold"))
        self.assertEqual(json.loads(self.wp("layers", "--json").stdout), [[2, 3], [4, 6]])
        dot = self.wp("graph", "--dot").stdout
        self.assertIn("i2 -> i4", dot)

    def test_layers_fail_on_a_cycle(self) -> None:
        self.spec(1, "002.")
        self.spec(2, "001.")
        r = self.wp("layers")
        self.assertEqual(r.returncode, 1)
        self.assertIn("cycle", r.stderr)

    def test_propose_splits_shared_areas_and_tracks(self) -> None:
        self.chain()
        self.spec(7)
        areas = self.write("areas.yaml", """
specs:
  2: {track: backend, areas: [core]}
  3: {track: backend, areas: [core]}
  4: {track: backend, areas: [core]}
  6: {track: backend, areas: [other]}
  7: {track: console, areas: [ui]}
""")
        r = self.wp("propose", "--areas", areas)
        self.assertEqual(r.returncode, 0, r.stderr)
        draft = self.write("draft.yaml", r.stdout)
        ok = self.wp("validate", draft, "--areas", areas)
        self.assertEqual(ok.returncode, 0, ok.stderr)
        keys = [line.split("key: ")[1] for line in r.stdout.splitlines() if "key: " in line]
        self.assertEqual(sorted(keys), ["B1", "B2", "B3", "C1"])  # 2 and 3 share an area, so they split; console stays apart
        missing = self.write("few.yaml", "specs:\n  2: {track: backend, areas: [core]}\n")
        self.assertIn("no entry", self.wp("propose", "--areas", missing).stderr)
        excluded = self.wp("propose", "--areas", areas, "--exclude", "7")
        self.assertEqual(excluded.returncode, 0, excluded.stderr)
        self.assertIn("exclude:\n- 7", excluded.stdout)
        self.assertNotIn("key: C1", excluded.stdout)

    def test_validate_cases(self) -> None:
        self.chain()
        cases = [
            ("waves:\n  - {key: A, name: a, description: d, issues: [2, 3, 4, 6]}\n", 0, "same wave"),
            ("waves:\n  - {key: A, name: a, description: d, issues: [2, 3]}\n", 1, "#4 s4 is in no wave"),
            ("exclude: [4, 6]\nwaves:\n  - {key: A, name: a, description: d, issues: [2, 3]}\n", 0, "plan valid"),
            ("exclude: [6]\nwaves:\n  - {key: A, name: a, description: d, issues: [2, 3, 5]}\n  - {key: B, name: b, description: d, issues: [4], after: [A]}\n", 1, "on hold"),
            ("exclude: [6]\nwaves:\n  - {key: A, name: a, description: d, issues: [2, 3, 4]}\n  - {key: B, name: b, description: d, issues: [4], after: [A]}\n", 1, "in both"),
            ("exclude: [6]\nwaves:\n  - {key: A, name: a, description: d, issues: [3, 4]}\n  - {key: B, name: b, description: d, issues: [2]}\n", 1, "not an earlier wave"),
            ("waves:\n  - {key: A, name: a, description: d, issues: [2], after: [Z]}\n", 1, "unknown parent"),
            ("waves:\n  - {key: A, name: a, description: d, issues: [2], after: [B]}\n  - {key: B, name: b, description: d, issues: [3], after: [A]}\n", 1, "cycle"),
            ("waves:\n  - {key: A, name: a, description: d, issues: [2]}\n  - {key: A, name: b, description: d, issues: [3]}\n", 1, "appears twice"),
            ("waves: nonsense\n", 1, "cannot read"),
        ]
        for text, code, expected in cases:
            r = self.wp("validate", self.write("p.yaml", text))
            self.assertEqual(r.returncode, code, f"{text}\n{r.stderr}")
            self.assertIn(expected, r.stderr, text)

    def test_validate_warns_about_areas(self) -> None:
        self.chain()
        areas = self.write("areas.yaml", """
specs:
  2: {track: b, areas: [core]}
  3: {track: b, areas: [core]}
  4: {track: b, areas: [x]}
  6: {track: b, areas: [x]}
""")
        r = self.wp("validate", self.write("p.yaml", self.PLAN), "--areas", areas)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("[2, 3] share area core", r.stderr)
        self.assertIn("parallel waves B and C share areas ['x']", r.stderr)

    def test_apply_in_order_then_rerun_reuses(self) -> None:
        self.chain()
        plan = self.write("p.yaml", """
waves:
  - {key: C, name: third, description: D3, issues: [6], after: [A]}
  - {key: B, name: second, description: D2, issues: [4], after: [A]}
  - {key: A, name: first, description: D1, issues: [2, 3]}
""")
        dry = self.wp("apply", plan, "--dry-run")
        self.assertEqual(dry.returncode, 0, dry.stderr)
        self.assertEqual(sorted(self.load()["issues"]), ["1", "2", "3", "4", "5", "6"])
        r = self.wp("apply", plan)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(r.stdout.split("\n")[0], "A 200")
        issues = self.load()["issues"]
        self.assertEqual(issues["200"]["title"], "wave: first")
        self.assertIn("## After\n- #200", issues["201"]["body"])
        self.assertIn("## After\n- #200", issues["202"]["body"])
        self.ok("check")
        again = self.wp("apply", plan)
        self.assertEqual(again.returncode, 0, again.stderr)
        self.assertEqual(self.load()["next"], 203)

    def test_apply_stops_when_github_differs(self) -> None:
        self.chain()
        plan = self.write("p.yaml", self.PLAN)
        self.assertEqual(self.wp("apply", plan).returncode, 0)
        changed = self.write("q.yaml", """
waves:
  - {key: A, name: first, description: D1, issues: [2, 3, 6]}
  - {key: B, name: second, description: D2, issues: [4], after: [A]}
""")
        r = self.wp("apply", changed)
        self.assertEqual(r.returncode, 1)
        self.assertIn("different content", r.stderr)

    def test_apply_refuses_an_invalid_plan(self) -> None:
        self.chain()
        r = self.wp("apply", self.write("p.yaml", "waves:\n  - {key: A, name: a, description: d, issues: [2]}\n"))
        self.assertEqual(r.returncode, 1)
        self.assertNotIn("200", self.load()["issues"])


if __name__ == "__main__":
    unittest.main()
