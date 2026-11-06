#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = [
#     "pydantic>=2.7",
#     "pyyaml>=6.0",
# ]
# ///
"""Create, list, check and change the status of specs that live as GitHub issues plus folders in specs/.

What it does:
  Every spec is a GitHub issue with the label `spec`. The issue number is the spec's id and the issue
  holds its status. The repo holds only content, in a flat tree: specs/NNN-kebab-name/spec.md, where
  NNN is the issue number, zero padded to at least 3 digits. Each spec.md starts with a YAML header
  with exactly the keys id, name and description, matching its folder:
      ---
      id: 063
      name: github-issue-specs
      description: One line saying what the spec achieves
      ---
  Status comes from the issue:
      todo     open, no status label
      working  open, label status:working
      hold     open, label status:hold, reason in a comment
      done     closed as completed
      dropped  closed as not planned (or duplicate), reason in a comment
  Every command first validates the local tree and refuses to change anything while a problem exists;
  all problems are reported at once. A command that writes to GitHub also checks that the logged-in gh
  account can write to the repo.
    init NAME DESCRIPTION  create an issue, then specs/NNN-NAME/spec.md with a section skeleton
    init ... --issue N     turn existing open issue N into the spec instead of creating one
    working ID             todo -> working (several specs may be in working)
    hold ID REASON         todo or working -> hold, with the reason as an issue comment
    resume ID              hold -> todo
    done ID                working -> done; refused unless origin/main has the spec with a filled-in Result
    drop ID REASON         todo, working, hold or done -> dropped, with the reason as an issue comment
    list                   print all specs, including spec issues with no folder on this branch
    check                  validate the tree and compare it with GitHub (--offline: the tree only)
  ID may be given as 7 or 007.

Output:
  stdout  init: the path of the new spec.md. working/hold/resume/done/drop: the issue's URL. list: a table
          (or JSON with --json). check: nothing.
  stderr  Log lines: "YYYY-MM-DD HH:MM LEVEL message key=value ...".
          Levels: DEBUG (only with -v), INFO, WARN, ERROR (one per problem found), FAIL.

Exit codes:
  0  success
  1  failure (the tree is invalid, the issue is not in the needed state, gh or git failed, ...; a FAIL
     line on stderr says why)
  2  invalid command line (printed by argparse)

Side effects:
  Network calls through gh, and `git fetch origin main` for done. init creates one GitHub issue (or edits
  one with --issue), one folder and one spec.md; the others change labels, comments and open/closed
  state on one issue. --dry-run validates and logs what would happen, and only reads from GitHub. list
  and check are read-only.

Examples:
  uv run .claude/skills/gh-spec-driven/scripts/gh-spec-manager.py list
  uv run .claude/skills/gh-spec-driven/scripts/gh-spec-manager.py init open-webui-role "Always-on Open WebUI"
  uv run .claude/skills/gh-spec-driven/scripts/gh-spec-manager.py init crash-on-upload "Fix the upload crash" --issue 70
  uv run .claude/skills/gh-spec-driven/scripts/gh-spec-manager.py working 64
  uv run .claude/skills/gh-spec-driven/scripts/gh-spec-manager.py hold 64 "Needs a reboot; wait for the user's OK"
  uv run .claude/skills/gh-spec-driven/scripts/gh-spec-manager.py done 064 --dry-run
  uv run .claude/skills/gh-spec-driven/scripts/gh-spec-manager.py list --json | jq '.[] | select(.status == "todo")'
  uv run .claude/skills/gh-spec-driven/scripts/gh-spec-manager.py check --offline

--help output:
[BEGIN HELP]
  $ gh-spec-manager.py --help
  usage: gh-spec-manager.py [-h] [-v] [--root DIR]
                            {init,working,resume,done,hold,drop,list,check} ...

  Create, list, check and change the status of specs that live as GitHub issues plus
  folders in specs/.

  positional arguments:
    {init,working,resume,done,hold,drop,list,check}
      init                create an issue and its spec folder
      working             move a spec from todo to working
      resume              move a spec from hold back to todo
      done                close a working spec as completed, once origin/main has its
                          Result
      hold                move a spec from todo or working to hold, noting why
      drop                close a spec as not planned, noting why
      list                print all specs
      check               validate the tree and compare it with GitHub

  options:
    -h, --help            show this help message and exit
    -v, --verbose         log DEBUG lines too (default: False)
    --root DIR            flat spec folder (default: <git repo root>/specs)

  $ gh-spec-manager.py init --help
  usage: gh-spec-manager.py init [-h] [--issue N] [--dry-run] NAME DESCRIPTION

  positional arguments:
    NAME         kebab-case name, e.g. open-webui-role
    DESCRIPTION  one-line description

  options:
    -h, --help   show this help message and exit
    --issue N    turn existing open issue N into the spec (default: None)
    --dry-run    validate and log, change nothing (default: False)

  $ gh-spec-manager.py working --help
  usage: gh-spec-manager.py working [-h] [--dry-run] ID

  positional arguments:
    ID          spec id, e.g. 7 or 007

  options:
    -h, --help  show this help message and exit
    --dry-run   validate and log, change nothing (default: False)

  $ gh-spec-manager.py resume --help
  usage: gh-spec-manager.py resume [-h] [--dry-run] ID

  positional arguments:
    ID          spec id, e.g. 7 or 007

  options:
    -h, --help  show this help message and exit
    --dry-run   validate and log, change nothing (default: False)

  $ gh-spec-manager.py done --help
  usage: gh-spec-manager.py done [-h] [--dry-run] ID

  positional arguments:
    ID          spec id, e.g. 7 or 007

  options:
    -h, --help  show this help message and exit
    --dry-run   validate and log, change nothing (default: False)

  $ gh-spec-manager.py hold --help
  usage: gh-spec-manager.py hold [-h] [--dry-run] ID REASON

  positional arguments:
    ID          spec id, e.g. 7 or 007
    REASON      one line: why it waits and for what (posted as an issue comment)

  options:
    -h, --help  show this help message and exit
    --dry-run   validate and log, change nothing (default: False)

  $ gh-spec-manager.py drop --help
  usage: gh-spec-manager.py drop [-h] [--dry-run] ID REASON

  positional arguments:
    ID          spec id, e.g. 7 or 007
    REASON      one line: why it was dropped (posted as an issue comment)

  options:
    -h, --help  show this help message and exit
    --dry-run   validate and log, change nothing (default: False)

  $ gh-spec-manager.py list --help
  usage: gh-spec-manager.py list [-h] [--json]

  options:
    -h, --help  show this help message and exit
    --json      print JSON instead of a table (default: False)

  $ gh-spec-manager.py check --help
  usage: gh-spec-manager.py check [-h] [--offline]

  options:
    -h, --help  show this help message and exit
    --offline   validate the tree only, without GitHub (default: False)
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
from pathlib import Path
from typing import Literal, Protocol

import yaml
from pydantic import BaseModel, ConfigDict, ValidationError, field_validator

log = logging.getLogger("gh-spec-manager")


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

Status = Literal["todo", "working", "hold", "done", "dropped"]
SPEC_LABEL = "spec"
IDEA_LABEL = "idea"
WAVE_LABEL = "wave"
WORKING_LABEL = "status:working"
HOLD_LABEL = "status:hold"
STATE_MARKS = ("> **On hold", "> **Dropped")
WRITE_PERMISSIONS = frozenset({"ADMIN", "MAINTAIN", "WRITE"})

NAME_RE = re.compile(r"^[a-z0-9]+(-[a-z0-9]+)*$")
ID_RE = re.compile(r"^\d{3,}$")
FOLDER_RE = re.compile(r"^(\d{3,})-(.+)$")
ISSUE_URL_RE = re.compile(r"/issues/(\d+)\s*$")
HEADER_KEYS = ("id", "name", "description")
TOLERATED_FILES = frozenset({".DS_Store", ".gitkeep"})
RESULT_PLACEHOLDER = "(Filled in when the spec moves to done or dropped"


class SpecHeader(BaseModel):
    """The YAML header of a spec.md; all values are strings (parsed with yaml.BaseLoader)."""

    model_config = ConfigDict(frozen=True, extra="forbid")

    id: str
    name: str
    description: str

    @field_validator("id")
    @classmethod
    def _id(cls, v: str) -> str:
        if not ID_RE.match(v):
            raise ValueError(f"id must be at least 3 digits, zero padded (e.g. 001), got {v!r}")
        return v

    @field_validator("name")
    @classmethod
    def _name(cls, v: str) -> str:
        if not NAME_RE.match(v):
            raise ValueError(f"name must be lowercase kebab-case (a-z, 0-9, single dashes), got {v!r}")
        return v

    @field_validator("description")
    @classmethod
    def _description(cls, v: str) -> str:
        if not v.strip() or "\n" in v.strip():
            raise ValueError("description must be one non-empty line")
        return v.strip()


class Entry(BaseModel):
    """One item found directly under specs/ (filled by the IO edge, judged by the core)."""

    model_config = ConfigDict(frozen=True)

    name: str
    is_dir: bool
    spec_text: str | None  # spec.md contents, None if missing or unreadable


class Spec(BaseModel):
    """A valid spec folder."""

    model_config = ConfigDict(frozen=True)

    id: str
    name: str
    description: str
    folder: str


class Issue(BaseModel):
    """The fields of a GitHub issue this script uses, as `gh issue ... --json` returns them."""

    model_config = ConfigDict(frozen=True)

    number: int
    title: str
    state: str  # OPEN or CLOSED
    stateReason: str | None = None  # COMPLETED, NOT_PLANNED, DUPLICATE, REOPENED, or empty
    url: str
    labels: list[str]


class Row(BaseModel):
    """One line of `list`: a spec folder joined with its issue. Either side can be missing."""

    model_config = ConfigDict(frozen=True)

    id: str
    name: str
    description: str
    status: str  # a Status, or "no-issue" for a folder without one
    folder: str | None
    url: str | None


class Plan(BaseModel):
    """What a status command will do to one issue. `apply` does exactly this, in this order."""

    model_config = ConfigDict(frozen=True)

    number: int
    reopen: bool = False
    add_labels: list[str] = []
    remove_labels: list[str] = []
    comment: str | None = None
    close_reason: str | None = None  # "completed" or "not planned"


class GitHub(Protocol):
    """All GitHub IO. The real one shells out to gh; tests replace gh itself with a fake on PATH."""

    def repo(self) -> tuple[str, str]: ...  # (owner/name, viewer permission)
    def list_spec_issues(self) -> list[Issue]: ...
    def get_issue(self, number: int) -> Issue | None: ...
    def create_issue(self, title: str, body: str, label: str) -> int: ...
    def edit_issue(self, number: int, *, title: str | None = None, body: str | None = None,
                   add_labels: list[str] | None = None, remove_labels: list[str] | None = None) -> None: ...
    def comment(self, number: int, body: str) -> None: ...
    def reopen(self, number: int) -> None: ...
    def close(self, number: int, reason: str, comment: str | None) -> None: ...


# --------------------------------------------------------------------------- pure core


def parse_header(text: str) -> SpecHeader:
    """Parse the `---` delimited YAML header at the top of a spec.md; raises ValueError with a reason."""
    lines = text.splitlines()
    if not lines or lines[0].strip() != "---":
        raise ValueError("spec.md must start with a '---' line opening the YAML header")
    try:
        end = next(i for i in range(1, len(lines)) if lines[i].strip() == "---")
    except StopIteration:
        raise ValueError("YAML header is not closed by a '---' line") from None
    try:
        raw = yaml.load("\n".join(lines[1:end]), Loader=yaml.BaseLoader)  # noqa: S506 - BaseLoader builds only str/list/dict
    except yaml.YAMLError as e:
        raise ValueError(f"YAML header does not parse: {str(e).splitlines()[0]}") from None
    if not isinstance(raw, dict):
        raise ValueError("YAML header must be a mapping of id, name, description")
    missing = [k for k in HEADER_KEYS if k not in raw]
    extra = sorted(k for k in raw if k not in HEADER_KEYS)
    if missing or extra:
        raise ValueError(f"YAML header needs exactly {list(HEADER_KEYS)} (missing {missing}, unexpected {extra})")
    if not all(isinstance(raw[k], str) for k in HEADER_KEYS):
        raise ValueError("YAML header values must be plain scalars")
    try:
        return SpecHeader.model_validate(raw)
    except ValidationError as e:
        raise ValueError("; ".join(err["msg"].removeprefix("Value error, ") for err in e.errors())) from None


def has_state_note(text: str) -> bool:
    """True if a status note sits between the title and the first section; status belongs on the issue."""
    for line in text.splitlines():
        if line.startswith("## "):
            return False
        if line.startswith(STATE_MARKS):
            return True
    return False


def validate(entries: list[Entry]) -> tuple[list[Spec], list[str]]:
    """Judge a scanned tree. Returns the valid specs and every problem found (empty = tree is valid)."""
    problems: list[str] = []
    specs: list[Spec] = []
    for e in entries:
        if not e.is_dir:
            if e.name not in TOLERATED_FILES:
                problems.append(f"{e.name}: unexpected file in specs/ (specs are folders)")
            continue
        m = FOLDER_RE.match(e.name)
        if not m or not NAME_RE.match(m.group(2)):
            problems.append(f"{e.name}: folder name must be NNN-kebab-name, where NNN is the issue number")
            continue
        if e.spec_text is None:
            problems.append(f"{e.name}: spec.md is missing or unreadable")
            continue
        try:
            h = parse_header(e.spec_text)
        except ValueError as err:
            problems.append(f"{e.name}/spec.md: {err}")
            continue
        if h.id != m.group(1):
            problems.append(f"{e.name}/spec.md: header id {h.id} does not match the folder id {m.group(1)}")
            continue
        if h.name != m.group(2):
            problems.append(f"{e.name}/spec.md: header name {h.name} does not match the folder name {m.group(2)}")
            continue
        if has_state_note(e.spec_text):
            problems.append(f"{e.name}/spec.md: remove the status note under the title; status lives on the issue")
            continue
        specs.append(Spec(id=h.id, name=h.name, description=h.description, folder=e.name))
    seen_ids: dict[int, Spec] = {}
    seen_names: dict[str, Spec] = {}
    for s in specs:
        if int(s.id) in seen_ids:
            problems.append(f"duplicate id: {seen_ids[int(s.id)].folder} and {s.folder}")
        if s.name in seen_names:
            problems.append(f"duplicate name: {seen_names[s.name].folder} and {s.folder}")
        seen_ids[int(s.id)] = s
        seen_names[s.name] = s
    specs.sort(key=lambda s: int(s.id))
    return specs, problems


def issue_status(issue: Issue) -> Status:
    if issue.state == "CLOSED":
        return "dropped" if issue.stateReason in ("NOT_PLANNED", "DUPLICATE") else "done"
    if HOLD_LABEL in issue.labels:
        return "hold"
    return "working" if WORKING_LABEL in issue.labels else "todo"


def issue_problems(specs: list[Spec], issues: list[Issue]) -> tuple[list[str], list[Issue]]:
    """Compare the tree with GitHub. Returns the problems, and spec issues with no folder on this branch."""
    problems: list[str] = []
    by_number = {i.number: i for i in issues}
    numbers = {int(s.id) for s in specs}
    for s in specs:
        issue = by_number.get(int(s.id))
        if issue is None:
            problems.append(f"{s.folder}: no issue #{int(s.id)} with the label {SPEC_LABEL}")
        elif not issue.title.startswith(f"{s.name}:"):
            problems.append(f"{s.folder}: issue #{issue.number} title must start with '{s.name}:', got {issue.title!r}")
    for i in issues:
        both = WORKING_LABEL in i.labels and HOLD_LABEL in i.labels
        closed_with_label = i.state == "CLOSED" and (WORKING_LABEL in i.labels or HOLD_LABEL in i.labels)
        if both:
            problems.append(f"issue #{i.number}: has both {WORKING_LABEL} and {HOLD_LABEL}")
        if closed_with_label:
            problems.append(f"issue #{i.number}: is closed but still has a status label")
    return problems, [i for i in issues if i.number not in numbers]


def split_title(title: str) -> tuple[str, str]:
    """Split `name: description`; a title in another form gives no name and the whole title."""
    name, sep, description = title.partition(":")
    return (name, description.strip()) if sep and NAME_RE.match(name) else ("", title)


def join_rows(specs: list[Spec], issues: list[Issue]) -> list[Row]:
    by_number = {i.number: i for i in issues}
    rows: list[Row] = []
    for s in specs:
        issue = by_number.get(int(s.id))
        rows.append(Row(id=s.id, name=s.name, description=s.description, folder=s.folder,
                        status=issue_status(issue) if issue else "no-issue", url=issue.url if issue else None))
    numbers = {int(s.id) for s in specs}
    for i in issues:
        if i.number not in numbers:
            name, description = split_title(i.title)
            rows.append(Row(id=f"{i.number:03d}", name=name, description=description, status=issue_status(i),
                            folder=None, url=i.url))
    rows.sort(key=lambda r: int(r.id))
    return rows


def render_table(rows: list[Row]) -> str:
    if not rows:
        return "(no specs)"
    w = max(len(r.name) for r in rows)
    lines = [f"{'ID':<4} {'STATUS':<8} {'NAME':<{w}}  DESCRIPTION"]
    for r in rows:
        note = "" if r.folder else "  (no folder on this branch)"
        lines.append(f"{r.id:<4} {r.status:<8} {r.name:<{w}}  {r.description}{note}")
    return "\n".join(lines)


def parse_id(raw: str) -> int:
    if not raw.isdigit() or int(raw) < 1:
        raise ScriptFailure(f"spec id must be a positive number {kv(got=raw)}; e.g. 7 or 007")
    return int(raw)


def one_line(reason: str | None, why: str) -> str:
    text = (reason or "").strip()
    if not text or "\n" in text:
        raise ScriptFailure(f"reason must be one non-empty line saying {why}")
    return text


def result_filled(text: str) -> bool:
    """True if spec.md has a `## Result` section with content other than the skeleton's placeholder."""
    lines = text.splitlines()
    try:
        start = lines.index("## Result") + 1
    except ValueError:
        return False
    body: list[str] = []
    for line in lines[start:]:
        if line.startswith("## "):
            break
        body.append(line)
    content = "\n".join(body).strip()
    return bool(content) and not content.startswith(RESULT_PLACEHOLDER)


def wrong_status(issue: Issue, command: str, needs: tuple[Status, ...]) -> ScriptFailure:
    status = issue_status(issue)
    hint = f"; `resume {issue.number}` first (needs the user's OK)" if status == "hold" and command != "drop" else ""
    return ScriptFailure(f"`{command}` needs a spec in {'/'.join(needs)} {kv(id=issue.number, status=status)}{hint}")


def plan_status(command: str, issue: Issue, reason: str | None, date: str) -> Plan:
    """Decide what `working`, `hold`, `resume`, `done` or `drop` does to this issue."""
    status = issue_status(issue)
    n = issue.number
    if SPEC_LABEL not in issue.labels:
        raise ScriptFailure(f"issue is not a spec {kv(id=n, title=issue.title)}; it has no label {SPEC_LABEL}")
    if command == "working":
        if status != "todo":
            raise wrong_status(issue, command, ("todo",))
        return Plan(number=n, add_labels=[WORKING_LABEL])
    if command == "hold":
        if status not in ("todo", "working"):
            raise wrong_status(issue, command, ("todo", "working"))
        why = one_line(reason, "why it waits and for what")
        return Plan(number=n, add_labels=[HOLD_LABEL], remove_labels=[WORKING_LABEL] if status == "working" else [],
                    comment=f"**On hold ({date}):** {why}")
    if command == "resume":
        if status != "hold":
            raise wrong_status(issue, command, ("hold",))
        return Plan(number=n, remove_labels=[HOLD_LABEL])
    if command == "done":
        if status != "working":
            raise wrong_status(issue, command, ("working",))
        return Plan(number=n, remove_labels=[WORKING_LABEL], close_reason="completed")
    if command == "drop":
        if status == "dropped":
            raise wrong_status(issue, command, ("todo", "working", "hold", "done"))
        why = one_line(reason, "why it was dropped")
        remove = [label for label in (WORKING_LABEL, HOLD_LABEL) if label in issue.labels]
        return Plan(number=n, reopen=status == "done", remove_labels=remove, close_reason="not planned",
                    comment=f"**Dropped ({date}):** {why}")
    raise ScriptFailure(f"unknown status command {kv(command=command)}")


def needs_yaml_quotes(s: str) -> bool:
    return bool(re.search(r"[:#\[\]{}&*!|>'\"%@`]", s)) or s[:1] in "-?,"


def render_spec_md(spec_id: str, name: str, description: str) -> str:
    return f"""---
id: {spec_id}
name: {name}
description: {json.dumps(description, ensure_ascii=False) if needs_yaml_quotes(description) else description}
---

# {spec_id} {name}

**Depends on:** none.

## Goal
What changes and why it matters (the expected outcome, with numbers where they exist).

## Background
Context, measurements and requirements that motivate it (links to docs, earlier specs, user requests).

## Approach
How it will be done: files, components, steps. Diagrams (mermaid / graphviz) can live next to this file.

## Verification
The tests and the concrete check that show it works or decide keep/drop.

## Out of scope
What this spec deliberately does not do.

## Result
{RESULT_PLACEHOLDER}: what was built, decisions, numbers, follow-ups.)
"""


def spec_url(repo: str, folder: str) -> str:
    return f"https://github.com/{repo}/blob/main/specs/{folder}/spec.md"


def issue_body(description: str, repo: str, folder: str) -> str:
    return f"{description}\n\nSpec: [specs/{folder}/spec.md]({spec_url(repo, folder)})\n"


def check_init(specs: list[Spec], issues: list[Issue], name: str, description: str) -> str:
    """Validate init's arguments against the tree and GitHub; returns the cleaned description."""
    if not NAME_RE.match(name):
        raise ScriptFailure(f"name must be lowercase kebab-case {kv(name=name)}; e.g. open-webui-role")
    text = one_line(description, "what the spec achieves")
    if any(s.name == name for s in specs):
        raise ScriptFailure(f"a spec folder with this name exists {kv(name=name)}")
    taken = [i for i in issues if i.title.startswith(f"{name}:")]
    if taken:
        raise ScriptFailure(f"a spec issue with this name exists {kv(name=name, id=taken[0].number)}; it may be on another branch")
    return text


def check_adopt(issue: Issue | None, number: int) -> Issue:
    """An issue can become a spec if it exists, is open, and isn't a spec yet."""
    if issue is None:
        raise ScriptFailure(f"no such issue {kv(id=number)}")
    if issue.state != "OPEN":
        raise ScriptFailure(f"only an open issue can become a spec {kv(id=number, state=issue.state)}")
    if SPEC_LABEL in issue.labels:
        raise ScriptFailure(f"this issue is already a spec {kv(id=number, title=issue.title)}")
    if WAVE_LABEL in issue.labels:
        raise ScriptFailure(f"a wave issue can't become a spec {kv(id=number, title=issue.title)}")
    return issue


# --------------------------------------------------------------------------- io edge


class GhCli:
    """GitHub through the gh command line, run from the repo so gh picks the right remote."""

    FIELDS = "number,title,state,stateReason,url,labels"

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

    @staticmethod
    def _issue(raw: dict[str, object]) -> Issue:
        labels = raw.get("labels")
        names = [str(label["name"]) for label in labels] if isinstance(labels, list) else []
        return Issue.model_validate({**raw, "labels": names, "stateReason": raw.get("stateReason") or None})

    def repo(self) -> tuple[str, str]:
        raw = json.loads(self._run("repo", "view", "--json", "nameWithOwner,viewerPermission"))
        return str(raw["nameWithOwner"]), str(raw["viewerPermission"])

    def list_spec_issues(self) -> list[Issue]:
        out = self._run("issue", "list", "--state", "all", "--label", SPEC_LABEL, "--limit", "5000", "--json", self.FIELDS)
        return [self._issue(raw) for raw in json.loads(out)]

    def get_issue(self, number: int) -> Issue | None:
        env = {**os.environ, "GH_PROMPT_DISABLED": "1", "NO_COLOR": "1"}
        r = subprocess.run(["gh", "issue", "view", str(number), "--json", self.FIELDS], cwd=self.cwd,
                           capture_output=True, text=True, env=env, check=False)
        if r.returncode == 0:
            return self._issue(json.loads(r.stdout))
        if "Could not resolve" in r.stderr or "not found" in r.stderr.lower():
            return None
        raise ScriptFailure(f"gh failed {kv(args=f'issue view {number}', exit=r.returncode, stderr=r.stderr.strip()[:400])}")

    def create_issue(self, title: str, body: str, label: str) -> int:
        out = self._run("issue", "create", "--title", title, "--body", body, "--label", label)
        m = ISSUE_URL_RE.search(out)
        if not m:
            raise ScriptFailure(f"gh did not print an issue URL {kv(stdout=out.strip()[:200])}")
        return int(m.group(1))

    def edit_issue(self, number: int, *, title: str | None = None, body: str | None = None,
                   add_labels: list[str] | None = None, remove_labels: list[str] | None = None) -> None:
        args = ["issue", "edit", str(number)]
        if title is not None:
            args += ["--title", title]
        if body is not None:
            args += ["--body", body]
        for label in add_labels or []:
            args += ["--add-label", label]
        for label in remove_labels or []:
            args += ["--remove-label", label]
        self._run(*args)

    def comment(self, number: int, body: str) -> None:
        self._run("issue", "comment", str(number), "--body", body)

    def reopen(self, number: int) -> None:
        self._run("issue", "reopen", str(number))

    def close(self, number: int, reason: str, comment: str | None) -> None:
        args = ["issue", "close", str(number), "--reason", reason]
        if comment:
            args += ["--comment", comment]
        self._run(*args)


def scan_tree(root: Path) -> list[Entry]:
    if not root.is_dir():
        raise ScriptFailure(f"spec root is not a directory {kv(root=root)}")
    entries: list[Entry] = []
    for p in sorted(root.iterdir()):
        text: str | None = None
        if p.is_dir() and (p / "spec.md").is_file():
            try:
                text = (p / "spec.md").read_text(encoding="utf-8")
            except (OSError, UnicodeDecodeError) as e:
                log.debug("cannot read spec.md %s", kv(path=p / "spec.md", error=type(e).__name__))
        entries.append(Entry(name=p.name, is_dir=p.is_dir(), spec_text=text))
    log.debug("scanned spec tree %s", kv(root=root, entries=len(entries)))
    return entries


def spec_on_origin_main(root: Path, folder: str) -> str | None:
    """spec.md as it is on origin/main after a fetch, or None if it isn't there."""
    repo_dir = root.parent
    fetch = subprocess.run(["git", "fetch", "-q", "origin", "main"], cwd=repo_dir, capture_output=True, text=True, check=False)
    if fetch.returncode != 0:
        raise ScriptFailure(f"git fetch failed {kv(exit=fetch.returncode, stderr=fetch.stderr.strip()[:400])}")
    show = subprocess.run(["git", "show", f"origin/main:{root.name}/{folder}/spec.md"], cwd=repo_dir,
                          capture_output=True, text=True, check=False)
    return show.stdout if show.returncode == 0 else None


def default_root() -> Path:
    """<git repo root>/specs, or ./specs outside a git repo."""
    try:
        top = subprocess.run(["git", "rev-parse", "--show-toplevel"], capture_output=True, text=True, check=True)
    except (OSError, subprocess.CalledProcessError):
        return Path.cwd() / "specs"
    return Path(top.stdout.strip()) / "specs"


def require_valid(entries: list[Entry], root: Path) -> list[Spec]:
    specs, problems = validate(entries)
    for p in problems:
        log.error("invalid spec tree %s", kv(problem=p))
    if problems:
        raise ScriptFailure(f"spec tree is invalid {kv(problems=len(problems), root=root)}; fix the ERROR lines first, nothing was changed")
    log.debug("spec tree valid %s", kv(specs=len(specs)))
    return specs


def require_write(gh: GitHub) -> str:
    """The repo's owner/name, once the logged-in account is known to be able to write to it."""
    repo, permission = gh.repo()
    if permission not in WRITE_PERMISSIONS:
        raise ScriptFailure(f"the active gh account can't write to this repo {kv(repo=repo, permission=permission)}; try `gh auth switch`")
    return repo


def apply_plan(gh: GitHub, plan: Plan) -> None:
    if plan.reopen:
        gh.reopen(plan.number)
    if plan.add_labels or plan.remove_labels:
        gh.edit_issue(plan.number, add_labels=plan.add_labels, remove_labels=plan.remove_labels)
    if plan.close_reason:
        gh.close(plan.number, plan.close_reason, plan.comment)
    elif plan.comment:
        gh.comment(plan.number, plan.comment)


def run_init(root: Path, specs: list[Spec], gh: GitHub, name: str, description: str, adopt: int | None, dry: bool) -> None:
    issues = gh.list_spec_issues()
    text = check_init(specs, issues, name, description)
    title = f"{name}: {text}"
    existing = check_adopt(gh.get_issue(adopt), adopt) if adopt is not None else None
    if existing is not None and (root / f"{existing.number:03d}-{name}").exists():
        raise ScriptFailure(f"destination already exists {kv(folder=f'{existing.number:03d}-{name}')}")
    if dry:
        repo, _ = gh.repo()
        log.info("dry run, would %s", kv(action="adopt" if existing else "create", issue=adopt, title=title, repo=repo))
        return
    repo = require_write(gh)
    if existing is not None:
        number = existing.number
        folder = f"{number:03d}-{name}"
        gh.edit_issue(number, title=title, add_labels=[SPEC_LABEL], remove_labels=[IDEA_LABEL] if IDEA_LABEL in existing.labels else [])
        gh.comment(number, f"Spec: [specs/{folder}/spec.md]({spec_url(repo, folder)})")
    else:
        number = gh.create_issue(title, text, SPEC_LABEL)
        folder = f"{number:03d}-{name}"
        gh.edit_issue(number, body=issue_body(text, repo, folder))
    spec_md = root / folder / "spec.md"
    (root / folder).mkdir(parents=False, exist_ok=False)
    rendered = render_spec_md(f"{number:03d}", name, text)
    parse_header(rendered)  # what we write must pass our own validation
    spec_md.write_text(rendered, encoding="utf-8")
    log.info("created spec %s", kv(id=number, spec_md=spec_md))
    print(spec_md)


def run_status(root: Path, specs: list[Spec], gh: GitHub, command: str, raw_id: str, reason: str | None, dry: bool) -> None:
    number = parse_id(raw_id)
    issue = gh.get_issue(number)
    if issue is None:
        raise ScriptFailure(f"no such issue {kv(id=number)}")
    plan = plan_status(command, issue, reason, time.strftime("%Y-%m-%d"))
    if command == "done":
        folder = next((s.folder for s in specs if int(s.id) == number), None)
        if folder is None:
            raise ScriptFailure(f"no spec folder on this branch {kv(id=number)}; run this from a checkout that has it")
        text = spec_on_origin_main(root, folder)
        if text is None:
            raise ScriptFailure(f"origin/main doesn't have this spec yet {kv(folder=folder)}; merge and push before `done`")
        if not result_filled(text):
            raise ScriptFailure(f"the spec on origin/main has no filled-in Result {kv(folder=folder)}; write it and push before `done`")
    if dry:
        log.info("dry run, would change issue %s", kv(plan=plan))
        return
    require_write(gh)
    apply_plan(gh, plan)
    log.info("changed spec %s", kv(id=number, command=command, url=issue.url))
    print(issue.url)


def run_check(specs: list[Spec], gh: GitHub) -> None:
    issues = gh.list_spec_issues()
    problems, no_folder = issue_problems(specs, issues)
    for i in no_folder:
        log.warning("spec issue has no folder on this branch %s", kv(id=i.number, title=i.title))
    for p in problems:
        log.error("spec tree and GitHub disagree %s", kv(problem=p))
    if problems:
        raise ScriptFailure(f"spec tree and GitHub disagree {kv(problems=len(problems))}; fix the ERROR lines")
    log.info("issues match the tree %s", kv(issues=len(issues), no_folder=len(no_folder)))


# --------------------------------------------------------------------------- cli


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    fmt = argparse.ArgumentDefaultsHelpFormatter
    parser = argparse.ArgumentParser(description=(__doc__ or "").splitlines()[0], formatter_class=fmt)
    parser.add_argument("-v", "--verbose", action="store_true", help="log DEBUG lines too")
    parser.add_argument("--root", type=Path, default=argparse.SUPPRESS, metavar="DIR",
                        help="flat spec folder (default: <git repo root>/specs)")
    sub = parser.add_subparsers(dest="command", required=True)
    p = sub.add_parser("init", help="create an issue and its spec folder", formatter_class=fmt)
    p.add_argument("name", metavar="NAME", help="kebab-case name, e.g. open-webui-role")
    p.add_argument("description", metavar="DESCRIPTION", help="one-line description")
    p.add_argument("--issue", type=int, default=None, metavar="N", help="turn existing open issue N into the spec")
    p.add_argument("--dry-run", action="store_true", help="validate and log, change nothing")
    for cmd, text in (("working", "move a spec from todo to working"), ("resume", "move a spec from hold back to todo"),
                      ("done", "close a working spec as completed, once origin/main has its Result")):
        p = sub.add_parser(cmd, help=text, formatter_class=fmt)
        p.add_argument("id", metavar="ID", help="spec id, e.g. 7 or 007")
        p.add_argument("--dry-run", action="store_true", help="validate and log, change nothing")
    for cmd, text, reason in (("hold", "move a spec from todo or working to hold, noting why", "one line: why it waits and for what"),
                              ("drop", "close a spec as not planned, noting why", "one line: why it was dropped")):
        p = sub.add_parser(cmd, help=text, formatter_class=fmt)
        p.add_argument("id", metavar="ID", help="spec id, e.g. 7 or 007")
        p.add_argument("reason", metavar="REASON", help=f"{reason} (posted as an issue comment)")
        p.add_argument("--dry-run", action="store_true", help="validate and log, change nothing")
    p = sub.add_parser("list", help="print all specs", formatter_class=fmt)
    p.add_argument("--json", action="store_true", help="print JSON instead of a table")
    p = sub.add_parser("check", help="validate the tree and compare it with GitHub", formatter_class=fmt)
    p.add_argument("--offline", action="store_true", help="validate the tree only, without GitHub")
    return parser.parse_args(argv)


def main() -> int:
    args = parse_args()
    setup_logging(args.verbose)
    started = time.monotonic()
    root: Path = (args.root if "root" in args else default_root()).resolve()
    dry = getattr(args, "dry_run", False)
    log.info("starting %s", kv(command=args.command, root=root, dry_run=dry))
    try:
        specs = require_valid(scan_tree(root), root)
        if args.command == "check" and args.offline:
            log.info("spec tree valid %s", kv(specs=len(specs), elapsed_s=time.monotonic() - started))
            return 0
        gh: GitHub = GhCli(root.parent)
        if args.command == "check":
            run_check(specs, gh)
        elif args.command == "list":
            rows = join_rows(specs, gh.list_spec_issues())
            print(json.dumps([r.model_dump() for r in rows], indent=2) if args.json else render_table(rows))
        elif args.command == "init":
            run_init(root, specs, gh, args.name, args.description, args.issue, dry)
        else:
            run_status(root, specs, gh, args.command, args.id, getattr(args, "reason", None), dry)
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
