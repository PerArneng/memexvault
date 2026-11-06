#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = [
#     "pydantic>=2.7",
# ]
# ///
"""Show, check and change wave issues: GitHub issues that group work that is safe to do in parallel.

What it does:
  A wave is a GitHub issue with the label `wave` and a title starting `wave: `. Its body is a description, the
  issues in the wave, and the waves that must finish first. Several parents are allowed, so waves form a graph:
      <description>

      ## Issues
      - #23
      - #37

      ## After
      - #65
  A wave's status is computed from its members, never from its own open or closed flag:
      blocked  a parent wave isn't done
      ready    parents done, no member started
      active   parents done, some members working or closed
      held     parents done, and an open member is on hold (its children stay blocked)
      done     every member is closed (done or dropped)
    next         the waves that can start now, what to start in them, and hold specs waiting on the user
    list         every wave with its status and progress
    show ID      one wave: description, parents, children and each member's status
    gate ID      exit 0 and print the issues to start if its parents are done; exit 3 if not. --wait polls until open.
    dot          a Graphviz diagram of waves and their issues
    create       create a wave issue
    edit ID      change a wave's issues, parents or description
    sync         close waves whose members are all closed, and reopen closed waves that have open members
    check        validate the whole graph, including that each spec's `Depends on` line is met
  Depends on lines are read from specs/NNN-name/spec.md in this checkout.

Output:
  stdout  next/list/show: text, or JSON with --json. gate: one issue number per line. dot: the DOT source.
          create: the new wave's URL. edit: the wave's URL. sync, check: nothing.
  stderr  Log lines: "YYYY-MM-DD HH:MM LEVEL message key=value ...".
          Levels: DEBUG (only with -v), INFO, WARN, ERROR (one per problem found), FAIL.

Exit codes:
  0  success; for gate, the wave may start
  1  failure (a FAIL line on stderr says why)
  2  invalid command line (printed by argparse)
  3  gate only: the wave is blocked (with --wait: still blocked at the timeout)

Side effects:
  Reads issues through gh. create, edit and sync write to GitHub, and --dry-run on them only logs. next, list, show,
  gate, dot and check are read-only. gate --wait sleeps between polls.

Examples:
  uv run .claude/skills/gh-spec-driven/scripts/wave-manager.py next
  uv run .claude/skills/gh-spec-driven/scripts/wave-manager.py show 65
  uv run .claude/skills/gh-spec-driven/scripts/wave-manager.py gate 66 --wait --interval 300 --timeout 3600
  uv run .claude/skills/gh-spec-driven/scripts/wave-manager.py create "console shell" "Login and layout" --issues 40 --after 65
  uv run .claude/skills/gh-spec-driven/scripts/wave-manager.py edit 66 --add-issue 70 --remove-after 65
  uv run .claude/skills/gh-spec-driven/scripts/wave-manager.py dot --open-only | dot -Tsvg -o waves.svg

--help output:
[BEGIN HELP]
  $ wave-manager.py --help
  usage: wave-manager.py [-h] [-v] [--root DIR]
                         {next,list,show,gate,dot,create,edit,sync,check} ...

  Show, check and change wave issues: GitHub issues that group work that is safe to do
  in parallel.

  positional arguments:
    {next,list,show,gate,dot,create,edit,sync,check}
      next                show the waves that can start now
      list                print every wave
      show                show one wave
      gate                exit 0 and print what to start if the wave may start, exit 3
                          if not
      dot                 print a Graphviz diagram
      create              create a wave issue
      edit                change a wave's issues, parents or description
      sync                match each wave's open or closed flag to its members
      check               validate the wave graph

  options:
    -h, --help            show this help message and exit
    -v, --verbose         log DEBUG lines too (default: False)
    --root DIR            flat spec folder to read Depends on lines from (default: <git
                          repo root>/specs)

  $ wave-manager.py next --help
  usage: wave-manager.py next [-h] [--json]

  options:
    -h, --help  show this help message and exit
    --json      print JSON instead of text (default: False)

  $ wave-manager.py list --help
  usage: wave-manager.py list [-h] [--json]

  options:
    -h, --help  show this help message and exit
    --json      print JSON instead of text (default: False)

  $ wave-manager.py show --help
  usage: wave-manager.py show [-h] [--json] ID

  positional arguments:
    ID          wave issue number

  options:
    -h, --help  show this help message and exit
    --json      print JSON instead of text (default: False)

  $ wave-manager.py gate --help
  usage: wave-manager.py gate [-h] [--wait] [--interval S] [--timeout S] ID

  positional arguments:
    ID            wave issue number

  options:
    -h, --help    show this help message and exit
    --wait        poll until the wave may start (default: False)
    --interval S  seconds between polls with --wait (default: 300.0)
    --timeout S   give up with exit 3 after this many seconds; 0 waits forever (default:
                  0.0)

  $ wave-manager.py dot --help
  usage: wave-manager.py dot [-h] [--open-only] [--deps]

  options:
    -h, --help   show this help message and exit
    --open-only  leave out waves that are done (default: False)
    --deps       also draw spec Depends on edges (default: False)

  $ wave-manager.py create --help
  usage: wave-manager.py create [-h] --issues N [N ...] [--after N [N ...]] [--dry-run]
                                NAME DESCRIPTION

  positional arguments:
    NAME                the title after 'wave: '
    DESCRIPTION         what this wave does

  options:
    -h, --help          show this help message and exit
    --issues N [N ...]  member issue numbers
    --after N [N ...]   parent wave numbers (default: None)
    --dry-run           validate and log, change nothing (default: False)

  $ wave-manager.py edit --help
  usage: wave-manager.py edit [-h] [--add-issue N [N ...]] [--remove-issue N [N ...]]
                              [--add-after N [N ...]] [--remove-after N [N ...]]
                              [--description TEXT] [--dry-run]
                              ID

  positional arguments:
    ID                    wave issue number

  options:
    -h, --help            show this help message and exit
    --add-issue N [N ...]
                          issues to add (default: None)
    --remove-issue N [N ...]
                          issues to remove (default: None)
    --add-after N [N ...]
                          parent waves to add (default: None)
    --remove-after N [N ...]
                          parent waves to remove (default: None)
    --description TEXT    new description (default: None)
    --dry-run             validate and log, change nothing (default: False)

  $ wave-manager.py sync --help
  usage: wave-manager.py sync [-h] [--dry-run]

  options:
    -h, --help  show this help message and exit
    --dry-run   log what would change, change nothing (default: False)

  $ wave-manager.py check --help
  usage: wave-manager.py check [-h]

  options:
    -h, --help  show this help message and exit
[END HELP]
"""

from __future__ import annotations

import argparse
import json
import logging
import os
import re
import subprocess
import sys
import time
from collections.abc import Callable
from pathlib import Path
from typing import Protocol

from pydantic import BaseModel, ConfigDict

log = logging.getLogger("wave-manager")


# --------------------------------------------------------------------------- logging

# Rename the stdlib levels to the four-letter house style.
logging.addLevelName(logging.WARNING, "WARN")
logging.addLevelName(logging.CRITICAL, "FAIL")

_RESET = "\033[0m"
_KEY_STYLE = "\033[1;97m"  # bold bright white
_TIME_STYLE = "\033[2m"  # dim
_LEVEL_STYLES = {
    logging.DEBUG: "\033[36m",  # cyan
    logging.INFO: "\033[32m",  # green
    logging.WARNING: "\033[33m",  # yellow
    logging.ERROR: "\033[31m",  # red
    logging.CRITICAL: "\033[1;97;41m",  # bold white on red
}
# A `key=` token: start of message or after whitespace/punctuation that cannot be part of a key.
_KEY_RE = re.compile(r"(?<![\w.\-/=])([A-Za-z_][\w.\-]*)=")


class LogFormatter(logging.Formatter):
    """Formats `2021-01-12 15:56 INFO message key=value`, colored when writing to a TTY."""

    def __init__(self, color: bool) -> None:
        super().__init__(datefmt="%Y-%m-%d %H:%M")
        self.color = color

    def format(self, record: logging.LogRecord) -> str:
        stamp = self.formatTime(record, self.datefmt)
        level = record.levelname
        message = record.getMessage()
        if self.color:
            stamp = f"{_TIME_STYLE}{stamp}{_RESET}"
            level = f"{_LEVEL_STYLES.get(record.levelno, '')}{level}{_RESET}"
            message = _KEY_RE.sub(rf"{_KEY_STYLE}\1{_RESET}=", message)
        line = f"{stamp} {level} {message}"
        if record.exc_info:
            line += "\n" + self.formatException(record.exc_info)
        return line


def setup_logging(verbose: bool) -> None:
    handler = logging.StreamHandler(sys.stderr)
    color = sys.stderr.isatty() and "NO_COLOR" not in os.environ
    handler.setFormatter(LogFormatter(color))
    root = logging.getLogger()
    root.handlers[:] = [handler]
    root.setLevel(logging.DEBUG if verbose else logging.INFO)


def kv(**fields: object) -> str:
    """Render fields as `key=value` pairs; strings with spaces or quotes are JSON-quoted."""
    return " ".join(f"{key}={_fmt_value(value)}" for key, value in fields.items())


def _fmt_value(value: object) -> str:
    if isinstance(value, Path):
        value = str(value)
    if isinstance(value, str):
        needs_quotes = value == "" or any(c.isspace() or c in "\"'=" for c in value)
        return json.dumps(value, ensure_ascii=False) if needs_quotes else value
    if isinstance(value, float):
        return f"{value:.3f}".rstrip("0").rstrip(".")
    if isinstance(value, (list, tuple, set, frozenset)):
        return "[" + ",".join(_fmt_value(v) for v in value) + "]"
    if isinstance(value, BaseModel):
        return value.model_dump_json()
    return str(value)


class ScriptFailure(Exception):
    """Raised for an expected, explainable failure. main() logs it as FAIL and exits 1."""


# --------------------------------------------------------------------------- models

SPEC_LABEL = "spec"
WAVE_LABEL = "wave"
WORKING_LABEL = "status:working"
HOLD_LABEL = "status:hold"
TITLE_PREFIX = "wave: "
BLOCKED_EXIT = 3
WRITE_PERMISSIONS = frozenset({"ADMIN", "MAINTAIN", "WRITE"})
REF_RE = re.compile(r"^- #(\d+)\s*$")
ISSUE_URL_RE = re.compile(r"/issues/(\d+)\s*$")
KEBAB_RE = re.compile(r"^[a-z0-9]+(-[a-z0-9]+)*$")
SPEC_FOLDER_RE = re.compile(r"^(\d{3,})-")
EXCEPT_RE = re.compile(r"except(?:\s+the\s+\w+)?\s+(\d{3,})")
ID_OR_RANGE_RE = re.compile(r"(\d{3,})(?:-(\d{3,}))?")


class Issue(BaseModel):
    """The fields of a GitHub issue this script uses."""

    model_config = ConfigDict(frozen=True)

    number: int
    title: str
    state: str  # OPEN or CLOSED
    stateReason: str | None = None
    url: str
    labels: list[str]
    body: str = ""


class Wave(BaseModel):
    """A wave issue with its body parsed."""

    model_config = ConfigDict(frozen=True)

    number: int
    name: str
    description: str
    issues: list[int]
    after: list[int]
    closed: bool
    url: str


class Graph(BaseModel):
    """Every issue, the waves that parsed, and the problems found while parsing the rest."""

    model_config = ConfigDict(frozen=True)

    issues: dict[int, Issue]
    waves: dict[int, Wave]
    malformed: list[str]


class Member(BaseModel):
    model_config = ConfigDict(frozen=True)

    number: int
    name: str
    status: str
    url: str | None


class WaveView(BaseModel):
    """A wave as next, list and show present it."""

    model_config = ConfigDict(frozen=True)

    number: int
    name: str
    description: str
    url: str
    status: str
    done: int
    total: int
    after: list[int]
    waiting_for: list[int]
    children: list[int]
    members: list[Member]


class NextReport(BaseModel):
    model_config = ConfigDict(frozen=True)

    waves: list[WaveView]
    hold_outside_waves: list[Member]
    open_outside_waves: list[Member]
    message: str


class GitHub(Protocol):
    """All GitHub IO. The real one shells out to gh; tests replace gh itself with a fake on PATH."""

    def repo(self) -> tuple[str, str]: ...
    def list_issues(self) -> list[Issue]: ...
    def create_issue(self, title: str, body: str, label: str) -> int: ...
    def set_body(self, number: int, body: str) -> None: ...
    def close(self, number: int) -> None: ...
    def reopen(self, number: int) -> None: ...


# --------------------------------------------------------------------------- pure core: parsing


def render_body(description: str, issues: list[int], after: list[int]) -> str:
    parts = [description.strip(), "", "## Issues", *[f"- #{n}" for n in issues]]
    if after:
        parts += ["", "## After", *[f"- #{n}" for n in after]]
    return "\n".join(parts) + "\n"


def parse_body(body: str) -> tuple[str, list[int], list[int]]:
    """Split a wave body into description, issues and after; raises ValueError with a reason."""
    description: list[str] = []
    sections: dict[str, list[int]] = {}
    current: str | None = None
    for raw in body.replace("\r\n", "\n").split("\n"):
        line = raw.rstrip()
        if line.startswith("## "):
            current = line[3:].strip()
            if current not in ("Issues", "After"):
                raise ValueError(f"unknown section {line!r}; only '## Issues' and '## After' are allowed")
            if current in sections:
                raise ValueError(f"section {line!r} appears twice")
            sections[current] = []
        elif current is None:
            description.append(line)
        elif line:
            m = REF_RE.match(line)
            if not m:
                raise ValueError(f"line {line!r} under '## {current}' must look like '- #123'")
            sections[current].append(int(m.group(1)))
    if "Issues" not in sections:
        raise ValueError("the '## Issues' section is missing")
    return "\n".join(description).strip(), sections["Issues"], sections.get("After", [])


def build_graph(issues: list[Issue]) -> Graph:
    waves: dict[int, Wave] = {}
    malformed: list[str] = []
    for i in issues:
        if WAVE_LABEL not in i.labels:
            continue
        if not i.title.startswith(TITLE_PREFIX):
            malformed.append(f"wave #{i.number}: title must start with {TITLE_PREFIX!r}, got {i.title!r}")
            continue
        try:
            description, members, after = parse_body(i.body)
        except ValueError as e:
            malformed.append(f"wave #{i.number}: {e}")
            continue
        waves[i.number] = Wave(number=i.number, name=i.title[len(TITLE_PREFIX):].strip(), description=description, issues=members,
                               after=after, closed=i.state == "CLOSED", url=i.url)
    return Graph(issues={i.number: i for i in issues}, waves=waves, malformed=malformed)


def parse_depends(spec_text: str) -> set[int]:
    """The ids on a spec's `**Depends on:**` line, from its first sentence. Understands ranges and `except`."""
    for line in spec_text.splitlines():
        if line.startswith("**Depends on:**"):
            sentence = line.split(":**", 1)[1].strip()
            sentence = re.split(r"\.(?:\s|$)", sentence, maxsplit=1)[0]
            excluded = {int(x) for x in EXCEPT_RE.findall(sentence)}
            found: set[int] = set()
            for m in ID_OR_RANGE_RE.finditer(EXCEPT_RE.sub("", sentence)):
                low, high = int(m.group(1)), int(m.group(2) or m.group(1))
                found.update(range(low, high + 1))
            return found - excluded
    return set()


# --------------------------------------------------------------------------- pure core: status


def issue_status(issue: Issue) -> str:
    if issue.state == "CLOSED":
        return "dropped" if issue.stateReason in ("NOT_PLANNED", "DUPLICATE") else "done"
    if HOLD_LABEL in issue.labels:
        return "hold"
    return "working" if WORKING_LABEL in issue.labels else "todo"


def short_name(issue: Issue) -> str:
    """A spec's name from `name: description`, or the title of any other issue."""
    name, sep, _ = issue.title.partition(":")
    return name if sep and KEBAB_RE.match(name) else issue.title


def member_view(graph: Graph, number: int) -> Member:
    issue = graph.issues.get(number)
    if issue is None:
        return Member(number=number, name="(no such issue)", status="missing", url=None)
    return Member(number=number, name=short_name(issue), status=issue_status(issue), url=issue.url)


def wave_done(graph: Graph, wave: Wave) -> bool:
    """Done means it has members and all are closed. Its own open or closed flag plays no part."""
    members = [graph.issues.get(n) for n in wave.issues]
    return bool(members) and all(m is not None and m.state == "CLOSED" for m in members)


def parent_done(graph: Graph, number: int) -> bool:
    """A parent that doesn't exist or isn't a valid wave counts as not done, so problems block rather than unblock."""
    parent = graph.waves.get(number)
    return parent is not None and wave_done(graph, parent)


def wave_status(graph: Graph, wave: Wave) -> str:
    if wave_done(graph, wave):
        return "done"
    if not all(parent_done(graph, n) for n in wave.after):
        return "blocked"
    statuses = [member_view(graph, n).status for n in wave.issues]
    if "hold" in statuses:
        return "held"
    return "active" if any(s in ("working", "done", "dropped") for s in statuses) else "ready"


def wave_view(graph: Graph, wave: Wave) -> WaveView:
    members = [member_view(graph, n) for n in wave.issues]
    return WaveView(number=wave.number, name=wave.name, description=wave.description, url=wave.url, status=wave_status(graph, wave),
                    done=sum(m.status in ("done", "dropped") for m in members), total=len(members), after=wave.after,
                    waiting_for=[n for n in wave.after if not parent_done(graph, n)],
                    children=sorted(w.number for w in graph.waves.values() if wave.number in w.after), members=members)


def ancestors(graph: Graph, wave: Wave) -> set[int]:
    seen: set[int] = set()
    stack = list(wave.after)
    while stack:
        n = stack.pop()
        if n in seen:
            continue
        seen.add(n)
        parent = graph.waves.get(n)
        if parent:
            stack.extend(parent.after)
    return seen


def next_report(graph: Graph) -> NextReport:
    views = [wave_view(graph, graph.waves[n]) for n in sorted(graph.waves)]
    can_start = [v for v in views if v.status in ("ready", "active", "held")]
    in_wave = {n for w in graph.waves.values() for n in w.issues}
    outside = [member_view(graph, i.number) for i in sorted(graph.issues.values(), key=lambda i: i.number)
               if SPEC_LABEL in i.labels and i.state == "OPEN" and i.number not in in_wave]
    if can_start:
        message = f"{len(can_start)} wave(s) can start or continue"
    elif not views:
        message = "there are no waves"
    elif all(v.status == "done" for v in views):
        message = "every wave is done"
    else:
        blocked = [v.number for v in views if v.status == "blocked"]
        message = f"no wave can start: {len(blocked)} blocked with nothing to wait for; run check, as this suggests a cycle or bad parent"
    return NextReport(waves=can_start, hold_outside_waves=[m for m in outside if m.status == "hold"],
                      open_outside_waves=[m for m in outside if m.status != "hold"], message=message)


def members_with(view: WaveView, status: str) -> list[Member]:
    return [m for m in view.members if m.status == status]


# --------------------------------------------------------------------------- pure core: validation


def find_cycle(waves: dict[int, Wave]) -> list[int] | None:
    state: dict[int, int] = {}  # 1 = on the current path, 2 = finished

    def visit(n: int, path: list[int]) -> list[int] | None:
        state[n] = 1
        for p in waves[n].after:
            if p not in waves:
                continue
            if state.get(p) == 1:
                return [*path[path.index(p):], n, p] if p in path else [n, p]
            if p not in state:
                found = visit(p, [*path, n])
                if found:
                    return found
        state[n] = 2
        return None

    for n in sorted(waves):
        if n not in state:
            found = visit(n, [])
            if found:
                return found
    return None


def structure_problems(graph: Graph) -> list[str]:
    problems = list(graph.malformed)
    owner: dict[int, int] = {}
    for w in sorted(graph.waves.values(), key=lambda x: x.number):
        where = f"wave #{w.number}"
        if not w.issues:
            problems.append(f"{where}: has no issues")
        if len(set(w.issues)) != len(w.issues):
            problems.append(f"{where}: lists an issue twice")
        if len(set(w.after)) != len(w.after):
            problems.append(f"{where}: lists a parent twice")
        for n in w.issues:
            if n not in graph.issues:
                problems.append(f"{where}: issue #{n} doesn't exist")
            elif n in graph.waves or WAVE_LABEL in graph.issues[n].labels:
                problems.append(f"{where}: issue #{n} is a wave, so it can only be a parent under '## After'")
            if n in owner and owner[n] != w.number:
                problems.append(f"{where}: issue #{n} is also in wave #{owner[n]}")
            owner.setdefault(n, w.number)
        for n in w.after:
            if n == w.number:
                problems.append(f"{where}: lists itself as a parent")
            elif n not in graph.waves:
                problems.append(f"{where}: parent #{n} isn't a valid wave")
        if w.closed and not wave_done(graph, w):
            problems.append(f"{where}: is closed but has open issues; run sync")
    cycle = find_cycle({k: v for k, v in graph.waves.items() if k not in v.after})
    if cycle:
        problems.append("waves form a cycle: " + " -> ".join(f"#{n}" for n in cycle))
    return problems


def dependency_findings(graph: Graph, depends: dict[int, set[int]]) -> tuple[list[str], list[str]]:
    """Problems and warnings from Depends on lines: each open member's dependencies must be closed or scheduled earlier."""
    problems: list[str] = []
    warnings: list[str] = []
    owner = {n: w for w in graph.waves.values() for n in w.issues}
    for w in sorted(graph.waves.values(), key=lambda x: x.number):
        before = {n for a in ancestors(graph, w) if a in graph.waves for n in graph.waves[a].issues}
        for n in w.issues:
            issue = graph.issues.get(n)
            if issue is None or issue.state == "CLOSED":
                continue
            for dep in sorted(depends.get(n, set()) - {n}):
                target = graph.issues.get(dep)
                if target is None:
                    problems.append(f"spec #{n} in wave #{w.number}: depends on #{dep}, which doesn't exist")
                elif target.state == "CLOSED":
                    if issue_status(target) == "dropped":
                        warnings.append(f"spec #{n} in wave #{w.number}: depends on #{dep}, which was dropped")
                elif dep in before:
                    continue
                elif dep in w.issues:
                    warnings.append(f"spec #{n} in wave #{w.number}: depends on #{dep} in the same wave, so they can't start together")
                else:
                    where = f"wave #{owner[dep].number}" if dep in owner else "no wave"
                    problems.append(f"spec #{n} in wave #{w.number}: depends on open #{dep} in {where}, which isn't an earlier wave")
    return problems, warnings


def outside_warnings(graph: Graph) -> list[str]:
    report = next_report(graph)
    return [f"open spec #{m.number} {m.name} is in no wave" for m in report.open_outside_waves]


def dropped_warnings(graph: Graph) -> list[str]:
    return [f"wave #{w.number}: member #{n} was dropped, so check that later waves don't need it"
            for w in sorted(graph.waves.values(), key=lambda x: x.number) for n in w.issues if member_view(graph, n).status == "dropped"]


# --------------------------------------------------------------------------- pure core: rendering


def one_line_member(m: Member) -> str:
    return f"#{m.number} {m.name}"


def render_view(v: WaveView) -> list[str]:
    lines = [f"wave #{v.number} {v.name}  [{v.status}]  {v.done}/{v.total} closed"]
    for label, status in (("start", "todo"), ("in progress", "working"), ("on hold", "hold"), ("missing", "missing")):
        chosen = members_with(v, status)
        if chosen:
            lines.append(f"  {label}: " + ", ".join(one_line_member(m) for m in chosen))
    return lines


def render_next(report: NextReport) -> str:
    lines: list[str] = []
    for v in report.waves:
        lines += render_view(v)
    if not report.waves:
        lines.append(report.message)
    if report.hold_outside_waves:
        lines += ["", "On hold, waiting on the user (in no wave):"]
        lines += [f"  {one_line_member(m)}" for m in report.hold_outside_waves]
    if report.open_outside_waves:
        lines += ["", "Open specs in no wave:"]
        lines += [f"  {one_line_member(m)} [{m.status}]" for m in report.open_outside_waves]
    return "\n".join(lines)


def render_list(views: list[WaveView]) -> str:
    if not views:
        return "(no waves)"
    lines = [f"{'ID':<5} {'STATUS':<8} {'CLOSED':<7} {'AFTER':<16} NAME"]
    for v in views:
        after = ",".join(f"#{n}" for n in v.after) or "-"
        lines.append(f"{'#' + str(v.number):<5} {v.status:<8} {f'{v.done}/{v.total}':<7} {after:<16} {v.name}")
    return "\n".join(lines)


def render_show(v: WaveView) -> str:
    lines = [f"wave #{v.number} {v.name}", f"status: {v.status}, {v.done}/{v.total} issues closed", v.url, "", v.description, "",
             "Issues:"]
    lines += [f"  #{m.number:<4} {m.status:<8} {m.name}" for m in v.members]
    lines += ["", "After: " + (", ".join(f"#{n}" for n in v.after) or "nothing")]
    if v.waiting_for:
        lines.append("Waiting for: " + ", ".join(f"#{n}" for n in v.waiting_for))
    lines.append("Then: " + (", ".join(f"#{n}" for n in v.children) or "nothing"))
    return "\n".join(lines)


FONT = "Helvetica Neue,Helvetica,Arial,sans-serif"
# fill, border and text for each issue status.
ISSUE_STYLES = {
    "todo": ("#FFFFFF", "#64748B", "#1E293B"),
    "working": ("#DBEAFE", "#1D4ED8", "#1E3A8A"),
    "hold": ("#FEF3C7", "#B45309", "#78350F"),
    "done": ("#DCFCE7", "#15803D", "#14532D"),
    "dropped": ("#F1F5F9", "#94A3B8", "#64748B"),
    "missing": ("#FEE2E2", "#B91C1C", "#7F1D1D"),
}
# border color and width for each wave status.
WAVE_STYLES = {
    "blocked": ("#CBD5E1", 1),
    "ready": ("#1D4ED8", 3),
    "active": ("#0E7490", 3),
    "held": ("#B45309", 3),
    "done": ("#15803D", 1),
}


def dot_text(s: str) -> str:
    return s.replace("\\", "\\\\").replace('"', '\\"')


def dot_html(s: str) -> str:
    return s.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


def render_dot(graph: Graph, open_only: bool, depends: dict[int, set[int]] | None) -> str:
    """Waves as clusters of issue nodes, with edges from parent to child wave and optional spec dependency edges."""
    views = [wave_view(graph, graph.waves[n]) for n in sorted(graph.waves)]
    shown = [v for v in views if not (open_only and v.status == "done") and v.members]
    numbers = {v.number for v in shown}
    out = ["digraph waves {",
           f'  graph [rankdir=LR, compound=true, newrank=true, bgcolor="white", nodesep=0.14, ranksep=0.55, pad=0.3, fontname="{FONT}", fontsize=11];',
           f'  node [shape=box, style="rounded,filled", fontname="{FONT}", fontsize=10, height=0.3, margin="0.1,0.05", penwidth=1];',
           '  edge [color="#475569", arrowsize=0.7, penwidth=1.3];']
    anchor: dict[int, str] = {}
    node_of: dict[int, str] = {}
    for v in shown:
        border, width = WAVE_STYLES[v.status]
        label = (f'<<font color="#0F172A"><b>#{v.number} {dot_html(v.name)}</b></font><br align="left"/>'
                 f'<font point-size="9" color="#475569">{v.status}, {v.done}/{v.total} closed</font><br align="left"/>>')
        out += [f"  subgraph cluster_{v.number} {{",
                f'    label={label}; labeljust=l; style="rounded,filled"; fillcolor="#F8FAFC"; color="{border}"; penwidth={width}; margin=10;',
                f'    tooltip="{dot_text(v.description)}";', f'    URL="{dot_text(v.url)}";']
        # Edges enter at the middle member, and one rank per wave stops Graphviz stretching a cluster across columns.
        anchor[v.number] = f"w{v.number}_i{v.members[len(v.members) // 2].number}"
        for m in v.members:
            fill, line, text = ISSUE_STYLES.get(m.status, ISSUE_STYLES["missing"])
            node = f"w{v.number}_i{m.number}"
            node_of.setdefault(m.number, node)
            url = f' URL="{dot_text(m.url)}", target="_blank",' if m.url else ""
            dashed = ', style="rounded,filled,dashed"' if m.status == "dropped" else ""
            out.append(f'    {node} [label="#{m.number} {dot_text(m.name)}", fillcolor="{fill}", color="{line}", fontcolor="{text}",{url} '
                       f'tooltip="{dot_text(m.status)}"{dashed}];')
        out.append("    { rank=same; " + " ".join(f"w{v.number}_i{m.number};" for m in v.members) + " }")
        out.append("  }")
    for v in shown:
        for parent in v.after:
            if parent in numbers:
                out.append(f"  {anchor[parent]} -> {anchor[v.number]} [ltail=cluster_{parent}, lhead=cluster_{v.number}];")
    if depends is not None:
        for number, deps in sorted(depends.items()):
            for dep in sorted(deps):
                if number in node_of and dep in node_of:
                    out.append(f'  {node_of[dep]} -> {node_of[number]} [style=dashed, color="#94A3B8", penwidth=0.8, arrowsize=0.5, constraint=false];')
    out += ["  subgraph cluster_legend {", f'    label=<<b>Legend</b><br align="left"/>>; labeljust=l; style="rounded"; color="#CBD5E1"; fontname="{FONT}"; margin=10;']
    for status in ("todo", "working", "hold", "done", "dropped"):
        fill, line, text = ISSUE_STYLES[status]
        out.append(f'    legend_{status} [label="{status}", fillcolor="{fill}", color="{line}", fontcolor="{text}"];')
    out += ["    { rank=same; legend_todo; legend_working; legend_hold; legend_done; legend_dropped; }", "  }", "}"]
    return "\n".join(out) + "\n"


# --------------------------------------------------------------------------- pure core: changes


def parse_numbers(raw: list[str] | None) -> list[int]:
    """Numbers from `23 37`, `23,37` or `#23`."""
    found: list[int] = []
    for chunk in raw or []:
        for piece in chunk.split(","):
            text = piece.strip().lstrip("#")
            if not text.isdigit():
                raise ScriptFailure(f"expected an issue number {kv(got=piece)}; e.g. 23 or 23,37")
            found.append(int(text))
    return found


def clean_description(text: str) -> str:
    cleaned = text.strip()
    if not cleaned:
        raise ScriptFailure("a wave needs a description")
    if any(line.startswith("## ") for line in cleaned.splitlines()):
        raise ScriptFailure("a wave description can't contain '## ' headings; they mark the Issues and After sections")
    return cleaned


def with_wave(graph: Graph, wave: Wave) -> Graph:
    return Graph(issues=graph.issues, waves={**graph.waves, wave.number: wave}, malformed=graph.malformed)


def require_no_new_problems(before: Graph, after: Graph, depends: dict[int, set[int]]) -> None:
    """Refuse a change that adds a problem. Problems that were already there don't block fixing them."""
    known = set(structure_problems(before)) | set(dependency_findings(before, depends)[0])
    new = [p for p in [*structure_problems(after), *dependency_findings(after, depends)[0]] if p not in known]
    if new:
        for p in new:
            log.error("this change would break the wave graph %s", kv(problem=p))
        raise ScriptFailure(f"refusing a change that adds problems {kv(problems=len(new))}; nothing was changed")


def apply_edit(wave: Wave, add_issues: list[int], remove_issues: list[int], add_after: list[int], remove_after: list[int],
               description: str | None) -> Wave:
    for label, removing, current in (("issue", remove_issues, wave.issues), ("parent", remove_after, wave.after)):
        missing = [n for n in removing if n not in current]
        if missing:
            raise ScriptFailure(f"can't remove what isn't there {kv(kind=label, wave=wave.number, numbers=missing)}")
    issues = [n for n in wave.issues if n not in remove_issues] + [n for n in add_issues if n not in wave.issues]
    after = [n for n in wave.after if n not in remove_after] + [n for n in add_after if n not in wave.after]
    text = clean_description(description) if description is not None else wave.description
    return wave.model_copy(update={"issues": issues, "after": after, "description": text})


def sync_actions(graph: Graph) -> tuple[list[int], list[int]]:
    """The waves to close and to reopen, so each wave's flag matches its members."""
    close = [w.number for w in graph.waves.values() if not w.closed and wave_done(graph, w)]
    reopen = [w.number for w in graph.waves.values() if w.closed and not wave_done(graph, w)]
    return sorted(close), sorted(reopen)


# --------------------------------------------------------------------------- io edge


class GhCli:
    """GitHub through the gh command line, run from the repo so gh picks the right remote."""

    def __init__(self, cwd: Path) -> None:
        self.cwd = cwd

    def _run(self, *args: str) -> str:
        started = time.monotonic()
        env = {**os.environ, "GH_PROMPT_DISABLED": "1", "NO_COLOR": "1"}
        try:
            r = subprocess.run(["gh", *args], cwd=self.cwd, capture_output=True, text=True, env=env, check=False)
        except OSError as e:
            raise ScriptFailure(f"cannot run gh {kv(error=type(e).__name__, detail=str(e))}; install it and run `gh auth login`") from None
        log.debug("gh %s", kv(args=" ".join(args[:3]), exit=r.returncode, elapsed_ms=int((time.monotonic() - started) * 1000)))
        if r.returncode != 0:
            raise ScriptFailure(f"gh failed {kv(args=' '.join(args[:3]), exit=r.returncode, stderr=r.stderr.strip()[:400])}")
        return r.stdout

    def repo(self) -> tuple[str, str]:
        raw = json.loads(self._run("repo", "view", "--json", "nameWithOwner,viewerPermission"))
        return str(raw["nameWithOwner"]), str(raw["viewerPermission"])

    def list_issues(self) -> list[Issue]:
        out = self._run("issue", "list", "--state", "all", "--limit", "5000", "--json", "number,title,state,stateReason,url,labels,body")
        issues = []
        for raw in json.loads(out):
            names = [str(label["name"]) for label in raw.get("labels") or []]
            issues.append(Issue.model_validate({**raw, "labels": names, "stateReason": raw.get("stateReason") or None}))
        return issues

    def create_issue(self, title: str, body: str, label: str) -> int:
        out = self._run("issue", "create", "--title", title, "--body", body, "--label", label)
        m = ISSUE_URL_RE.search(out)
        if not m:
            raise ScriptFailure(f"gh did not print an issue URL {kv(stdout=out.strip()[:200])}")
        return int(m.group(1))

    def set_body(self, number: int, body: str) -> None:
        self._run("issue", "edit", str(number), "--body", body)

    def close(self, number: int) -> None:
        self._run("issue", "close", str(number), "--reason", "completed")

    def reopen(self, number: int) -> None:
        self._run("issue", "reopen", str(number))


def read_depends(root: Path) -> dict[int, set[int]]:
    """Depends on lines from specs/NNN-name/spec.md. A checkout without a specs folder simply has none."""
    found: dict[int, set[int]] = {}
    if not root.is_dir():
        log.debug("no spec folder %s", kv(root=root))
        return found
    for spec_md in sorted(root.glob("*/spec.md")):
        m = SPEC_FOLDER_RE.match(spec_md.parent.name)
        if m:
            found[int(m.group(1))] = parse_depends(spec_md.read_text(encoding="utf-8"))
    return found


def default_root() -> Path:
    """<git repo root>/specs, or ./specs outside a git repo."""
    try:
        top = subprocess.run(["git", "rev-parse", "--show-toplevel"], capture_output=True, text=True, check=True)
    except (OSError, subprocess.CalledProcessError):
        return Path.cwd() / "specs"
    return Path(top.stdout.strip()) / "specs"


def load_graph(gh: GitHub) -> Graph:
    graph = build_graph(gh.list_issues())
    for p in graph.malformed:
        log.warning("skipping a malformed wave %s", kv(problem=p))
    log.debug("loaded issues %s", kv(issues=len(graph.issues), waves=len(graph.waves)))
    return graph


def require_write(gh: GitHub) -> None:
    repo, permission = gh.repo()
    if permission not in WRITE_PERMISSIONS:
        raise ScriptFailure(f"the active gh account can't write to this repo {kv(repo=repo, permission=permission)}; try `gh auth switch`")


def require_wave(graph: Graph, raw: str) -> Wave:
    numbers = parse_numbers([raw])
    if len(numbers) != 1:
        raise ScriptFailure(f"expected one wave number {kv(got=raw)}")
    wave = graph.waves.get(numbers[0])
    if wave is None:
        raise ScriptFailure(f"no valid wave with this number {kv(id=numbers[0], known=sorted(graph.waves))}")
    return wave


def run_gate(gh: GitHub, raw: str, wait: bool, interval_s: float, timeout_s: float, sleep: Callable[[float], None],
             now: Callable[[], float]) -> int:
    if interval_s <= 0 or timeout_s < 0:
        raise ScriptFailure(f"interval must be above 0 and timeout at least 0 {kv(interval_s=interval_s, timeout_s=timeout_s)}")
    started = now()
    while True:
        graph = load_graph(gh)
        wave = require_wave(graph, raw)
        view = wave_view(graph, wave)
        if view.status != "blocked":
            for m in members_with(view, "hold"):
                log.warning("member is on hold and needs the user %s", kv(id=m.number, name=m.name))
            if view.status == "done":
                log.info("wave is already done %s", kv(id=wave.number))
            start = members_with(view, "todo")
            log.info("wave may start %s", kv(id=wave.number, status=view.status, start=[m.number for m in start],
                                             working=[m.number for m in members_with(view, "working")]))
            for m in start:
                print(m.number)
            return 0
        log.info("wave is blocked %s", kv(id=wave.number, waiting_for=view.waiting_for))
        waited = now() - started
        if not wait or (timeout_s and waited >= timeout_s):
            if wait:
                log.warning("gave up waiting %s", kv(id=wave.number, waited_s=waited))
            return BLOCKED_EXIT
        sleep(interval_s)


def run_create(gh: GitHub, root: Path, name: str, description: str, issues: list[int], after: list[int], dry: bool) -> None:
    graph = load_graph(gh)
    clean_name = name.strip()
    if not clean_name or "\n" in clean_name:
        raise ScriptFailure("a wave name must be one non-empty line")
    text = clean_description(description)
    if not issues:
        raise ScriptFailure("a wave needs at least one issue")
    title = f"{TITLE_PREFIX}{clean_name}"
    same = [i.number for i in graph.issues.values() if i.title == title]
    if same:
        raise ScriptFailure(f"a wave with this title exists {kv(id=same[0], title=title)}")
    placeholder = max(graph.issues, default=0) + 1
    draft = Wave(number=placeholder, name=clean_name, description=text, issues=issues, after=after, closed=False, url="")
    require_no_new_problems(graph, with_wave(graph, draft), read_depends(root))
    body = render_body(text, issues, after)
    if dry:
        log.info("dry run, would create %s", kv(title=title, issues=issues, after=after))
        return
    require_write(gh)
    number = gh.create_issue(title, body, WAVE_LABEL)
    log.info("created wave %s", kv(id=number, title=title, issues=issues, after=after))
    print(gh_url_for(gh, number))


def gh_url_for(gh: GitHub, number: int) -> str:
    repo, _ = gh.repo()
    return f"https://github.com/{repo}/issues/{number}"


def run_edit(gh: GitHub, root: Path, args: argparse.Namespace) -> None:
    graph = load_graph(gh)
    wave = require_wave(graph, args.id)
    changed = apply_edit(wave, parse_numbers(args.add_issue), parse_numbers(args.remove_issue), parse_numbers(args.add_after),
                         parse_numbers(args.remove_after), args.description)
    if changed == wave:
        log.info("nothing to change %s", kv(id=wave.number))
        print(wave.url)
        return
    require_no_new_problems(graph, with_wave(graph, changed), read_depends(root))
    if args.dry_run:
        log.info("dry run, would change wave %s", kv(id=wave.number, issues=changed.issues, after=changed.after))
        return
    require_write(gh)
    gh.set_body(wave.number, render_body(changed.description, changed.issues, changed.after))
    log.info("changed wave %s", kv(id=wave.number, issues=changed.issues, after=changed.after))
    print(wave.url)


def run_sync(gh: GitHub, dry: bool) -> None:
    close, reopen = sync_actions(load_graph(gh))
    if dry:
        log.info("dry run, would sync %s", kv(close=close, reopen=reopen))
        return
    if close or reopen:
        require_write(gh)
    for n in close:
        gh.close(n)
        log.info("closed wave %s", kv(id=n))
    for n in reopen:
        gh.reopen(n)
        log.info("reopened wave %s", kv(id=n))
    log.info("synced %s", kv(closed=len(close), reopened=len(reopen)))


def run_check(gh: GitHub, root: Path) -> None:
    graph = load_graph(gh)
    dep_problems, dep_warnings = dependency_findings(graph, read_depends(root))
    for w in [*dep_warnings, *outside_warnings(graph), *dropped_warnings(graph)]:
        log.warning("%s", w)
    problems = [*structure_problems(graph), *dep_problems]
    for p in problems:
        log.error("invalid wave graph %s", kv(problem=p))
    if problems:
        raise ScriptFailure(f"the wave graph is invalid {kv(problems=len(problems))}; fix the ERROR lines")
    log.info("wave graph valid %s", kv(waves=len(graph.waves)))


# --------------------------------------------------------------------------- cli


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    fmt = argparse.ArgumentDefaultsHelpFormatter
    parser = argparse.ArgumentParser(description=(__doc__ or "").splitlines()[0], formatter_class=fmt)
    parser.add_argument("-v", "--verbose", action="store_true", help="log DEBUG lines too")
    parser.add_argument("--root", type=Path, default=argparse.SUPPRESS, metavar="DIR",
                        help="flat spec folder to read Depends on lines from (default: <git repo root>/specs)")
    sub = parser.add_subparsers(dest="command", required=True)
    for cmd, text in (("next", "show the waves that can start now"), ("list", "print every wave")):
        p = sub.add_parser(cmd, help=text, formatter_class=fmt)
        p.add_argument("--json", action="store_true", help="print JSON instead of text")
    p = sub.add_parser("show", help="show one wave", formatter_class=fmt)
    p.add_argument("id", metavar="ID", help="wave issue number")
    p.add_argument("--json", action="store_true", help="print JSON instead of text")
    p = sub.add_parser("gate", help="exit 0 and print what to start if the wave may start, exit 3 if not", formatter_class=fmt)
    p.add_argument("id", metavar="ID", help="wave issue number")
    p.add_argument("--wait", action="store_true", help="poll until the wave may start")
    p.add_argument("--interval", type=float, default=300.0, metavar="S", help="seconds between polls with --wait")
    p.add_argument("--timeout", type=float, default=0.0, metavar="S", help="give up with exit 3 after this many seconds; 0 waits forever")
    p = sub.add_parser("dot", help="print a Graphviz diagram", formatter_class=fmt)
    p.add_argument("--open-only", action="store_true", help="leave out waves that are done")
    p.add_argument("--deps", action="store_true", help="also draw spec Depends on edges")
    p = sub.add_parser("create", help="create a wave issue", formatter_class=fmt)
    p.add_argument("name", metavar="NAME", help="the title after 'wave: '")
    p.add_argument("description", metavar="DESCRIPTION", help="what this wave does")
    p.add_argument("--issues", nargs="+", required=True, metavar="N", help="member issue numbers")
    p.add_argument("--after", nargs="+", default=None, metavar="N", help="parent wave numbers")
    p.add_argument("--dry-run", action="store_true", help="validate and log, change nothing")
    p = sub.add_parser("edit", help="change a wave's issues, parents or description", formatter_class=fmt)
    p.add_argument("id", metavar="ID", help="wave issue number")
    p.add_argument("--add-issue", nargs="+", default=None, metavar="N", help="issues to add")
    p.add_argument("--remove-issue", nargs="+", default=None, metavar="N", help="issues to remove")
    p.add_argument("--add-after", nargs="+", default=None, metavar="N", help="parent waves to add")
    p.add_argument("--remove-after", nargs="+", default=None, metavar="N", help="parent waves to remove")
    p.add_argument("--description", default=None, metavar="TEXT", help="new description")
    p.add_argument("--dry-run", action="store_true", help="validate and log, change nothing")
    p = sub.add_parser("sync", help="match each wave's open or closed flag to its members", formatter_class=fmt)
    p.add_argument("--dry-run", action="store_true", help="log what would change, change nothing")
    sub.add_parser("check", help="validate the wave graph", formatter_class=fmt)
    return parser.parse_args(argv)


def main() -> int:
    args = parse_args()
    setup_logging(args.verbose)
    started = time.monotonic()
    root: Path = (args.root if "root" in args else default_root()).resolve()
    log.info("starting %s", kv(command=args.command, root=root, dry_run=getattr(args, "dry_run", False)))
    gh: GitHub = GhCli(root.parent if root.parent.is_dir() else Path.cwd())
    try:
        if args.command == "gate":
            return run_gate(gh, args.id, args.wait, args.interval, args.timeout, time.sleep, time.monotonic)
        if args.command == "next":
            report = next_report(load_graph(gh))
            print(report.model_dump_json(indent=2) if args.json else render_next(report))
        elif args.command == "list":
            graph = load_graph(gh)
            views = [wave_view(graph, graph.waves[n]) for n in sorted(graph.waves)]
            print(json.dumps([v.model_dump() for v in views], indent=2) if args.json else render_list(views))
        elif args.command == "show":
            graph = load_graph(gh)
            view = wave_view(graph, require_wave(graph, args.id))
            print(view.model_dump_json(indent=2) if args.json else render_show(view))
        elif args.command == "dot":
            print(render_dot(load_graph(gh), args.open_only, read_depends(root) if args.deps else None), end="")
        elif args.command == "create":
            run_create(gh, root, args.name, args.description, parse_numbers(args.issues), parse_numbers(args.after), args.dry_run)
        elif args.command == "edit":
            run_edit(gh, root, args)
        elif args.command == "sync":
            run_sync(gh, args.dry_run)
        elif args.command == "check":
            run_check(gh, root)
    except ScriptFailure as e:
        log.critical("%s", e)
        return 1
    except OSError as e:
        log.critical("filesystem operation failed %s", kv(error=type(e).__name__, detail=str(e)))
        return 1
    log.info("done %s", kv(elapsed_s=time.monotonic() - started))
    return 0


if __name__ == "__main__":
    sys.exit(main())
