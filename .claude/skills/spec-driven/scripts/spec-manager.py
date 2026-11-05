#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = [
#     "pydantic>=2.7",
#     "pyyaml>=6.0",
# ]
# ///
"""Create, list, check and move a repo's specs between todo, working, hold, done and dropped.

What it does:
  Manages the spec tree of the git repo you run it in (--root to override):
  specs/{todo,working,hold,done,dropped}/NNN-kebab-name/spec.md.
  Each spec.md starts with a YAML header with exactly the keys id, name and description:
      ---
      id: 001
      name: open-webui-role
      description: One line saying what the spec achieves
      ---
  The header must match its folder (id = the NNN prefix, name = the rest). Other files in a spec
  folder (mermaid diagrams, data, notes) are allowed. Every command first validates the whole tree
  (folder names, spec.md present, header valid, ids and names unique, at most one spec in working)
  and refuses to change anything while a problem exists; all problems are reported at once.
  A spec in dropped must say why right under its title, before the first `## ` section, in a line
  starting with `> **Dropped` (the drop command writes it). A spec in hold likewise needs a
  `> **On hold` line there (the hold command writes it, resume removes it).
    init NAME DESCRIPTION  create todo/NNN-NAME/spec.md (NNN = highest id anywhere + 1) with a
                           section skeleton to fill in
    working ID             move a spec from todo to working (refused if working holds a spec)
    done ID                move a spec from working to done
    hold ID REASON         move a spec from todo or working to hold (parked until the user says
                           go) and write `> **On hold (YYYY-MM-DD):** REASON` under its title
    resume ID              move a spec from hold back to todo and remove its on-hold line
    drop ID REASON         move a spec from todo, working, hold or done (re-filed) to dropped and
                           write `> **Dropped (YYYY-MM-DD):** REASON` under its title (an on-hold
                           line is removed)
    list                   print all specs
    check                  validate only
  ID may be given as 7 or 007. Moves use rename (atomic on one filesystem) and the tree is
  validated again afterwards.

Output:
  stdout  init: the path of the new spec.md. working/done/hold/resume/drop: the spec's new folder. list: a table
          (or JSON with --json). check: nothing.
  stderr  Log lines: "YYYY-MM-DD HH:MM LEVEL message key=value ...".
          Levels: DEBUG (only with -v), INFO, WARN, ERROR (one per problem found), FAIL.

Exit codes:
  0  success
  1  failure (the tree is invalid, the spec is not where the command needs it, ...; a FAIL line
     on stderr says why and nothing was changed)
  2  invalid command line (printed by argparse)

Side effects:
  init creates one folder and one spec.md; working/done rename one folder; hold/resume/drop
  rewrite the spec.md (add or remove the note line) and rename its folder. --dry-run validates and
  logs what would happen without changing anything. list and check are read-only.

Examples:
  uv run .claude/skills/spec-driven/scripts/spec-manager.py init open-webui-role "Always-on Open WebUI that follows the running model"
  uv run .claude/skills/spec-driven/scripts/spec-manager.py working 1
  uv run .claude/skills/spec-driven/scripts/spec-manager.py done 001 --dry-run
  uv run .claude/skills/spec-driven/scripts/spec-manager.py hold 6 "Needs a reboot; wait for the user's OK"
  uv run .claude/skills/spec-driven/scripts/spec-manager.py resume 6
  uv run .claude/skills/spec-driven/scripts/spec-manager.py drop 2 "The stock image was fast enough; a custom build is not needed"
  uv run .claude/skills/spec-driven/scripts/spec-manager.py list --json | jq '.[] | select(.status == "todo")'
  uv run .claude/skills/spec-driven/scripts/spec-manager.py check

--help output:
[BEGIN HELP]
  $ spec-manager.py --help
  usage: spec-manager.py [-h] [-v] [--root DIR]
                         {init,working,done,hold,resume,drop,list,check} ...

  Create, list, check and move a repo's specs between todo, working, hold, done and
  dropped.

  positional arguments:
    {init,working,done,hold,resume,drop,list,check}
      init                create a new spec in todo
      working             move a spec from todo to working
      done                move a spec from working to done
      hold                move a spec from todo or working to hold, noting why
      resume              move a spec from hold back to todo
      drop                move a spec to dropped, noting why
      list                print all specs
      check               validate the spec tree only

  options:
    -h, --help            show this help message and exit
    -v, --verbose         log DEBUG lines too (default: False)
    --root DIR            spec root holding todo/, working/, hold/, done/ and dropped/
                          (default: <git repo root>/specs)

  $ spec-manager.py init --help
  usage: spec-manager.py init [-h] [--dry-run] NAME DESCRIPTION

  positional arguments:
    NAME         kebab-case name, e.g. open-webui-role
    DESCRIPTION  one-line description

  options:
    -h, --help   show this help message and exit
    --dry-run    validate and log, change nothing (default: False)

  $ spec-manager.py working --help
  usage: spec-manager.py working [-h] [--dry-run] ID

  positional arguments:
    ID          spec id, e.g. 7 or 007

  options:
    -h, --help  show this help message and exit
    --dry-run   validate and log, change nothing (default: False)

  $ spec-manager.py done --help
  usage: spec-manager.py done [-h] [--dry-run] ID

  positional arguments:
    ID          spec id, e.g. 7 or 007

  options:
    -h, --help  show this help message and exit
    --dry-run   validate and log, change nothing (default: False)

  $ spec-manager.py hold --help
  usage: spec-manager.py hold [-h] [--dry-run] ID REASON

  positional arguments:
    ID          spec id, e.g. 7 or 007
    REASON      one line: why it waits and for what (written under the spec's title)

  options:
    -h, --help  show this help message and exit
    --dry-run   validate and log, change nothing (default: False)

  $ spec-manager.py resume --help
  usage: spec-manager.py resume [-h] [--dry-run] ID

  positional arguments:
    ID          spec id, e.g. 7 or 007

  options:
    -h, --help  show this help message and exit
    --dry-run   validate and log, change nothing (default: False)

  $ spec-manager.py drop --help
  usage: spec-manager.py drop [-h] [--dry-run] ID REASON

  positional arguments:
    ID          spec id, e.g. 7 or 007
    REASON      one line: why it was dropped (written under the spec's title)

  options:
    -h, --help  show this help message and exit
    --dry-run   validate and log, change nothing (default: False)

  $ spec-manager.py list --help
  usage: spec-manager.py list [-h] [--json]

  options:
    -h, --help  show this help message and exit
    --json      print JSON instead of a table (default: False)

  $ spec-manager.py check --help
  usage: spec-manager.py check [-h]

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
from typing import Literal

import yaml
from pydantic import BaseModel, ConfigDict, ValidationError, field_validator

log = logging.getLogger("spec-manager")


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
STATUSES: tuple[Status, ...] = ("todo", "working", "hold", "done", "dropped")
DROPPED_MARK = "> **Dropped"
HOLD_MARK = "> **On hold"
# Statuses whose spec.md must carry a note line under the title, and its mark.
NOTE_MARKS: dict[Status, str] = {"dropped": DROPPED_MARK, "hold": HOLD_MARK}

NAME_RE = re.compile(r"^[a-z0-9]+(-[a-z0-9]+)*$")
ID_RE = re.compile(r"^\d{3,}$")
FOLDER_RE = re.compile(r"^(\d{3,})-(.+)$")
HEADER_KEYS = ("id", "name", "description")
TOLERATED_FILES = frozenset({".gitkeep", ".DS_Store"})


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
    """One item found in a status folder (filled by the IO edge, judged by the core)."""

    model_config = ConfigDict(frozen=True)

    status: Status
    name: str
    is_dir: bool
    spec_text: str | None  # spec.md contents, None if missing or unreadable


class Scan(BaseModel):
    model_config = ConfigDict(frozen=True)

    root: Path
    missing_status_dirs: list[Status]
    entries: list[Entry]


class Spec(BaseModel):
    model_config = ConfigDict(frozen=True)

    id: str
    name: str
    description: str
    status: Status
    folder: str


class Move(BaseModel):
    model_config = ConfigDict(frozen=True)

    spec: Spec
    src: Path
    dst: Path


class Rewrite(BaseModel):
    """A move that also rewrites spec.md (hold, resume, drop)."""

    model_config = ConfigDict(frozen=True)

    move: Move
    spec_md: Path  # in the source folder, rewritten before the move
    text: str


class Creation(BaseModel):
    model_config = ConfigDict(frozen=True)

    folder: Path
    spec_md: Path
    text: str


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


def validate(scan: Scan) -> tuple[list[Spec], list[str]]:
    """Judge a scanned tree. Returns the valid specs and every problem found (empty = tree is valid)."""
    problems = [f"status folder {s}/ is missing under {scan.root}" for s in scan.missing_status_dirs]
    specs: list[Spec] = []
    for e in scan.entries:
        where = f"{e.status}/{e.name}"
        if not e.is_dir:
            if e.name not in TOLERATED_FILES:
                problems.append(f"{where}: unexpected file in a status folder (specs are folders)")
            continue
        m = FOLDER_RE.match(e.name)
        if not m or not NAME_RE.match(m.group(2)):
            problems.append(f"{where}: folder name must be NNN-kebab-name (e.g. 001-open-webui-role)")
            continue
        if e.spec_text is None:
            problems.append(f"{where}: spec.md is missing or unreadable")
            continue
        try:
            h = parse_header(e.spec_text)
        except ValueError as err:
            problems.append(f"{where}/spec.md: {err}")
            continue
        if h.id != m.group(1):
            problems.append(f"{where}/spec.md: header id {h.id} does not match the folder id {m.group(1)}")
            continue
        if h.name != m.group(2):
            problems.append(f"{where}/spec.md: header name {h.name} does not match the folder name {m.group(2)}")
            continue
        mark = NOTE_MARKS.get(e.status)
        if mark and not has_note(e.spec_text, mark):
            problems.append(f"{where}/spec.md: a {e.status} spec needs a '{mark} (date):** why' line under its title, "
                            f"before the first '## ' section")
            continue
        specs.append(Spec(id=h.id, name=h.name, description=h.description, status=e.status, folder=e.name))
    for key, label in ((lambda s: int(s.id), "id"), (lambda s: s.name, "name")):
        seen: dict[object, Spec] = {}
        for s in specs:
            k: object = key(s)
            if k in seen:
                problems.append(f"duplicate {label}: {seen[k].status}/{seen[k].folder} and {s.status}/{s.folder}")
            seen[k] = s
    working = [s for s in specs if s.status == "working"]
    if len(working) > 1:
        problems.append(f"more than one spec in working: {[s.folder for s in working]}")
    specs.sort(key=lambda s: int(s.id))
    return specs, problems


def body_lines(text: str) -> tuple[list[str], int]:
    """The lines of a spec.md and the index of the first line after the YAML header."""
    lines = text.splitlines()
    end = next(i for i in range(1, len(lines)) if lines[i].strip() == "---")
    return lines, end + 1


def note_index(text: str, mark: str) -> int | None:
    """Line index of the note starting with `mark` between the header and the first `## ` section."""
    lines, start = body_lines(text)
    for i in range(start, len(lines)):
        if lines[i].startswith("## "):
            return None
        if lines[i].startswith(mark):
            return i
    return None


def has_note(text: str, mark: str) -> bool:
    return note_index(text, mark) is not None


def add_note(text: str, mark: str, reason: str, date: str) -> str:
    """Insert `mark (date):** reason` after the `# ` title (or right after the header if there is none)."""
    lines, start = body_lines(text)
    at = next((i + 1 for i in range(start, len(lines)) if lines[i].startswith("# ")), start)
    note = ["", f"{mark} ({date}):** {reason}"]
    out = lines[:at] + note + lines[at:]
    return "\n".join(out) + "\n"


def remove_note(text: str, mark: str) -> str:
    """Remove the note line starting with `mark` and the blank line add_note put in front of it."""
    i = note_index(text, mark)
    if i is None:
        return text
    lines, _ = body_lines(text)
    lo = i - 1 if i > 0 and not lines[i - 1].strip() else i
    return "\n".join(lines[:lo] + lines[i + 1:]) + "\n"


def require_valid(scan: Scan) -> list[Spec]:
    specs, problems = validate(scan)
    for p in problems:
        log.error("invalid spec tree %s", kv(problem=p))
    if problems:
        raise ScriptFailure(f"spec tree is invalid {kv(problems=len(problems), root=scan.root)}; fix the ERROR lines first, nothing was changed")
    log.debug("spec tree valid %s", kv(specs=len(specs)))
    return specs


def normalize_id(raw: str) -> str:
    if not raw.isdigit():
        raise ScriptFailure(f"spec id must be a number {kv(got=raw)}; e.g. 7 or 007")
    return f"{int(raw):03d}"


def find(specs: list[Spec], spec_id: str) -> Spec:
    hits = [s for s in specs if int(s.id) == int(spec_id)]
    if len(hits) != 1:
        raise ScriptFailure(f"no spec with this id {kv(id=spec_id, known=[s.id for s in specs])}")
    return hits[0]


def plan_move(root: Path, specs: list[Spec], spec_id: str, to: Status, exists: Callable[[Path], bool]) -> Move:
    spec = find(specs, normalize_id(spec_id))
    need: Status = "todo" if to == "working" else "working"
    if spec.status != need:
        hint = f"; it is on hold, `resume {spec.id}` first (needs the user's OK)" if spec.status == "hold" else ""
        raise ScriptFailure(f"spec is not in {need} {kv(id=spec.id, name=spec.name, status=spec.status, wanted=to)}; "
                            f"specs move todo -> working -> done (todo/working -> hold with `hold`, back with `resume`; "
                            f"-> dropped with `drop`){hint}")
    if to == "working":
        busy = [s for s in specs if s.status == "working"]
        if busy:
            raise ScriptFailure(f"working already holds a spec {kv(busy=busy[0].folder)}; finish it with `done {busy[0].id}` first")
    src, dst = root / spec.status / spec.folder, root / to / spec.folder
    if exists(dst):
        raise ScriptFailure(f"destination already exists {kv(dst=dst)}; nothing was moved")
    return Move(spec=spec, src=src, dst=dst)


# Where each rewriting command may take a spec from, and where it puts it.
REWRITES: dict[str, tuple[tuple[Status, ...], Status]] = {
    "hold": (("todo", "working"), "hold"),
    "resume": (("hold",), "todo"),
    "drop": (("todo", "working", "hold", "done"), "dropped"),
}


def plan_rewrite(root: Path, specs: list[Spec], command: str, spec_id: str, reason: str | None, date: str,
                 read_text: Callable[[Path], str], exists: Callable[[Path], bool]) -> Rewrite:
    """Plan hold (add the on-hold note), resume (remove it) or drop (add the dropped note, remove an on-hold note)."""
    sources, to = REWRITES[command]
    spec = find(specs, normalize_id(spec_id))
    if spec.status not in sources:
        raise ScriptFailure(f"`{command}` needs a spec in {'/'.join(sources)} "
                            f"{kv(id=spec.id, name=spec.name, status=spec.status)}")
    src, dst = root / spec.status / spec.folder, root / to / spec.folder
    if exists(dst):
        raise ScriptFailure(f"destination already exists {kv(dst=dst)}; nothing was moved")
    text = read_text(src / "spec.md")
    new = text
    if command in ("resume", "drop"):
        new = remove_note(new, HOLD_MARK)
    if command in ("hold", "drop"):
        mark = NOTE_MARKS[to]
        reason = (reason or "").strip()
        if not reason or "\n" in reason:
            raise ScriptFailure(f"reason must be one non-empty line saying why the spec goes to {to}")
        if has_note(new, mark):
            raise ScriptFailure(f"spec.md already has a '{mark}' line {kv(spec_md=src / 'spec.md')}")
        new = add_note(new, mark, reason, date)
    if parse_header(new) != parse_header(text) or (to in NOTE_MARKS and not has_note(new, NOTE_MARKS[to])) \
            or has_note(new, HOLD_MARK) != (to == "hold"):
        raise ScriptFailure("internal error: the rewritten spec.md does not validate; nothing was changed")
    return Rewrite(move=Move(spec=spec, src=src, dst=dst), spec_md=src / "spec.md", text=new)


def render_spec_md(spec_id: str, name: str, description: str) -> str:
    return f"""---
id: {spec_id}
name: {name}
description: {json.dumps(description, ensure_ascii=False) if needs_yaml_quotes(description) else description}
---

# {spec_id} {name}

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
(Filled in when the spec moves to done or dropped: what was built, decisions, numbers, follow-ups.)
"""


def needs_yaml_quotes(s: str) -> bool:
    return bool(re.search(r"[:#\[\]{}&*!|>'\"%@`]", s)) or s[:1] in "-?,"


def plan_init(root: Path, specs: list[Spec], name: str, description: str) -> Creation:
    if not NAME_RE.match(name):
        raise ScriptFailure(f"name must be lowercase kebab-case {kv(name=name)}; e.g. open-webui-role")
    description = description.strip()
    if not description or "\n" in description:
        raise ScriptFailure("description must be one non-empty line")
    taken = [s for s in specs if s.name == name]
    if taken:
        raise ScriptFailure(f"a spec with this name exists {kv(existing=f'{taken[0].status}/{taken[0].folder}')}")
    spec_id = f"{max((int(s.id) for s in specs), default=0) + 1:03d}"
    folder = root / "todo" / f"{spec_id}-{name}"
    text = render_spec_md(spec_id, name, description)
    parse_header(text)  # the header we write must pass our own validation
    return Creation(folder=folder, spec_md=folder / "spec.md", text=text)


def render_table(specs: list[Spec]) -> str:
    if not specs:
        return "(no specs)"
    w = max(len(s.name) for s in specs)
    rows = [f"{'ID':<4} {'STATUS':<8} {'NAME':<{w}}  DESCRIPTION"]
    rows += [f"{s.id:<4} {s.status:<8} {s.name:<{w}}  {s.description}" for s in specs]
    return "\n".join(rows)


# --------------------------------------------------------------------------- io edge


def scan_tree(root: Path) -> Scan:
    if not root.is_dir():
        raise ScriptFailure(f"spec root is not a directory {kv(root=root)}")
    missing: list[Status] = []
    entries: list[Entry] = []
    for status in STATUSES:
        d = root / status
        if not d.is_dir():
            missing.append(status)
            continue
        for p in sorted(d.iterdir()):
            text: str | None = None
            if p.is_dir() and (p / "spec.md").is_file():
                try:
                    text = (p / "spec.md").read_text(encoding="utf-8")
                except (OSError, UnicodeDecodeError) as e:
                    log.debug("cannot read spec.md %s", kv(path=p / "spec.md", error=type(e).__name__))
            entries.append(Entry(status=status, name=p.name, is_dir=p.is_dir(), spec_text=text))
    log.debug("scanned spec tree %s", kv(root=root, entries=len(entries), missing=missing))
    return Scan(root=root, missing_status_dirs=missing, entries=entries)


def create_spec(c: Creation) -> None:
    c.folder.mkdir(parents=False, exist_ok=False)
    c.spec_md.write_text(c.text, encoding="utf-8")


def rename(m: Move) -> None:
    m.src.rename(m.dst)


def read_text_file(p: Path) -> str:
    return p.read_text(encoding="utf-8")


def write_text_file(p: Path, text: str) -> None:
    p.write_text(text, encoding="utf-8")


# --------------------------------------------------------------------------- cli


def default_root() -> Path:
    """<git repo root>/specs, or ./specs outside a git repo."""
    try:
        top = subprocess.run(["git", "rev-parse", "--show-toplevel"], capture_output=True, text=True, check=True)
    except (OSError, subprocess.CalledProcessError):
        return Path.cwd() / "specs"
    return Path(top.stdout.strip()) / "specs"


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    fmt = argparse.ArgumentDefaultsHelpFormatter
    parser = argparse.ArgumentParser(description=(__doc__ or "").splitlines()[0], formatter_class=fmt)
    parser.add_argument("-v", "--verbose", action="store_true", help="log DEBUG lines too")
    parser.add_argument("--root", type=Path, default=argparse.SUPPRESS, metavar="DIR",
                        help="spec root holding todo/, working/, hold/, done/ and dropped/ (default: <git repo root>/specs)")
    sub = parser.add_subparsers(dest="command", required=True)
    p = sub.add_parser("init", help="create a new spec in todo", formatter_class=fmt)
    p.add_argument("name", metavar="NAME", help="kebab-case name, e.g. open-webui-role")
    p.add_argument("description", metavar="DESCRIPTION", help="one-line description")
    p.add_argument("--dry-run", action="store_true", help="validate and log, change nothing")
    for cmd, text in (("working", "move a spec from todo to working"), ("done", "move a spec from working to done")):
        p = sub.add_parser(cmd, help=text, formatter_class=fmt)
        p.add_argument("id", metavar="ID", help="spec id, e.g. 7 or 007")
        p.add_argument("--dry-run", action="store_true", help="validate and log, change nothing")
    p = sub.add_parser("hold", help="move a spec from todo or working to hold, noting why", formatter_class=fmt)
    p.add_argument("id", metavar="ID", help="spec id, e.g. 7 or 007")
    p.add_argument("reason", metavar="REASON", help="one line: why it waits and for what (written under the spec's title)")
    p.add_argument("--dry-run", action="store_true", help="validate and log, change nothing")
    p = sub.add_parser("resume", help="move a spec from hold back to todo", formatter_class=fmt)
    p.add_argument("id", metavar="ID", help="spec id, e.g. 7 or 007")
    p.add_argument("--dry-run", action="store_true", help="validate and log, change nothing")
    p = sub.add_parser("drop", help="move a spec to dropped, noting why", formatter_class=fmt)
    p.add_argument("id", metavar="ID", help="spec id, e.g. 7 or 007")
    p.add_argument("reason", metavar="REASON", help="one line: why it was dropped (written under the spec's title)")
    p.add_argument("--dry-run", action="store_true", help="validate and log, change nothing")
    p = sub.add_parser("list", help="print all specs", formatter_class=fmt)
    p.add_argument("--json", action="store_true", help="print JSON instead of a table")
    sub.add_parser("check", help="validate the spec tree only", formatter_class=fmt)
    return parser.parse_args(argv)


def main() -> int:
    args = parse_args()
    setup_logging(args.verbose)
    started = time.monotonic()
    root: Path = (args.root if "root" in args else default_root()).resolve()
    dry = getattr(args, "dry_run", False)
    log.info("starting %s", kv(command=args.command, root=root, dry_run=dry))
    try:
        specs = require_valid(scan_tree(root))
        if args.command == "check":
            log.info("spec tree valid %s", kv(specs=len(specs), elapsed_s=time.monotonic() - started))
            return 0
        if args.command == "list":
            print(json.dumps([s.model_dump() for s in specs], indent=2) if args.json else render_table(specs))
            return 0
        if args.command == "init":
            c = plan_init(root, specs, args.name, args.description)
            if dry:
                log.info("dry run, would create %s", kv(spec_md=c.spec_md))
                return 0
            create_spec(c)
            log.info("created spec %s", kv(spec_md=c.spec_md))
            print(c.spec_md)
        elif args.command in REWRITES:
            d = plan_rewrite(root, specs, args.command, args.id, getattr(args, "reason", None), time.strftime("%Y-%m-%d"),
                             read_text=read_text_file, exists=lambda p: p.exists())
            if dry:
                log.info("dry run, would rewrite spec.md and move %s", kv(src=d.move.src, dst=d.move.dst))
                return 0
            write_text_file(d.spec_md, d.text)
            rename(d.move)
            log.info("moved spec %s", kv(id=d.move.spec.id, name=d.move.spec.name, command=args.command, dst=d.move.dst))
            print(d.move.dst)
        else:
            m = plan_move(root, specs, args.id, args.command, exists=lambda p: p.exists())
            if dry:
                log.info("dry run, would move %s", kv(src=m.src, dst=m.dst))
                return 0
            rename(m)
            log.info("moved spec %s", kv(id=m.spec.id, name=m.spec.name, to=args.command, dst=m.dst))
            print(m.dst)
        require_valid(scan_tree(root))
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
