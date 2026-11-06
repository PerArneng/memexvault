#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = []
# ///
"""Create one GitHub issue per spec, in id order, so that issue #N is spec N. One-off, for spec 063.

What it does:
  Reads a `spec-manager.py list --json` snapshot and the old status folders. It creates the labels, creates the
  missing issues in id order, and stops at once if GitHub returns a number other than the spec's id, since numbers
  can't be reused. It then applies each spec's status: labels, reason comments, and closing done and dropped specs.
  A rerun skips what exists, so an interrupted run continues without duplicates.

Output:
  stdout  Nothing.
  stderr  Log lines: "YYYY-MM-DD HH:MM LEVEL message key=value ...".

Exit codes:
  0  success
  1  failure (a FAIL line says why; rerun only after reading it)
  2  invalid command line

Side effects:
  Creates labels and issues on the public repo, adds comments and closes issues. --dry-run only reads.

Examples:
  uv run specs/063-github-issue-specs/tools/migrate.py --snapshot /path/specs.json --dry-run
  uv run specs/063-github-issue-specs/tools/migrate.py --snapshot /path/specs.json
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
import time
from pathlib import Path

REPO = "PerArneng/memexvault"
LOGIN = "PerArneng"
LABELS = {
    "spec": ("1d76db", "A spec: its content is in specs/<id>-<name>/"),
    "status:working": ("fbca04", "Spec in progress"),
    "status:hold": ("d93f0b", "Spec waiting for the user; the reason is in a comment"),
    "idea": ("c5def5", "An idea for a future spec, with no folder yet"),
}
NOTE_RE = re.compile(r"^> \*\*(On hold|Dropped) \((\d{4}-\d{2}-\d{2})\):\*\* (.+)$", re.M)
PAUSE_S = 1.5


def log(level: str, message: str) -> None:
    print(f"{time.strftime('%Y-%m-%d %H:%M')} {level} {message}", file=sys.stderr, flush=True)


class Stop(Exception):
    pass


def gh(*args: str) -> str:
    r = subprocess.run(["gh", *args], capture_output=True, text=True, check=False)
    if r.returncode != 0:
        raise Stop(f"gh failed args={' '.join(args[:3])} stderr={r.stderr.strip()[:400]}")
    return r.stdout


def reason_of(root: Path, spec: dict[str, str]) -> str:
    """`**On hold (date):** why` from the old note line, for hold and dropped specs."""
    text = (root / spec["status"] / spec["folder"] / "spec.md").read_text(encoding="utf-8")
    m = NOTE_RE.search(text)
    if not m:
        raise Stop(f"no status note found spec={spec['folder']}")
    return f"**{m.group(1)} ({m.group(2)}):** {m.group(3)}"


def title_of(spec: dict[str, str]) -> str:
    return f"{spec['name']}: {spec['description']}"


def body_of(spec: dict[str, str]) -> str:
    path = f"specs/{spec['folder']}/spec.md"
    return f"{spec['description']}\n\nSpec: [{path}](https://github.com/{REPO}/blob/main/{path})\n"


def highest_number() -> int:
    """The newest issue or PR number visible; PRs share the sequence. Deleted ones are caught when creating."""
    raw = json.loads(gh("api", f"repos/{REPO}/issues?state=all&per_page=1&sort=created&direction=desc"))
    return int(raw[0]["number"]) if raw else 0


def create_issues(specs: list[dict[str, str]], dry: bool) -> None:
    highest = highest_number()
    log("INFO", f"creating issues highest_existing={highest} specs={len(specs)} dry_run={dry}")
    for spec in specs:
        n = int(spec["id"])
        if n <= highest:
            title = json.loads(gh("issue", "view", str(n), "--repo", REPO, "--json", "title"))["title"]
            if title != title_of(spec):
                raise Stop(f"issue exists with another title id={n} title={title!r}")
            log("INFO", f"exists id={n}")
            continue
        if dry:
            log("INFO", f"would create id={n} title={title_of(spec)!r}")
            continue
        out = gh("issue", "create", "--repo", REPO, "--title", title_of(spec), "--body", body_of(spec), "--label", "spec")
        got = int(out.strip().rsplit("/", 1)[-1])
        if got != n:
            raise Stop(f"NUMBER MISMATCH expected={n} got={got} url={out.strip()}; nothing further was created")
        log("INFO", f"created id={n}")
        time.sleep(PAUSE_S)


def apply_status(root: Path, specs: list[dict[str, str]], dry: bool) -> None:
    for spec in specs:
        n, status = spec["id"], spec["status"]
        if status == "todo":
            continue
        number = str(int(n))
        reason = reason_of(root, spec) if status in ("hold", "dropped") else None
        if dry:
            log("INFO", f"would set id={n} status={status} comment={reason!r}")
            continue
        issue = json.loads(gh("issue", "view", number, "--repo", REPO, "--json", "state,labels"))
        labels = {label["name"] for label in issue["labels"]}
        if status == "working" and "status:working" not in labels:
            gh("issue", "edit", number, "--repo", REPO, "--add-label", "status:working")
        elif status == "hold" and "status:hold" not in labels:
            gh("issue", "comment", number, "--repo", REPO, "--body", reason or "")
            gh("issue", "edit", number, "--repo", REPO, "--add-label", "status:hold")
        elif status == "done" and issue["state"] == "OPEN":
            gh("issue", "close", number, "--repo", REPO, "--reason", "completed")
        elif status == "dropped" and issue["state"] == "OPEN":
            gh("issue", "close", number, "--repo", REPO, "--reason", "not planned", "--comment", reason or "")
        else:
            log("INFO", f"already set id={n} status={status}")
            continue
        log("INFO", f"set id={n} status={status}")
        time.sleep(PAUSE_S)


def main() -> int:
    parser = argparse.ArgumentParser(description=(__doc__ or "").splitlines()[0])
    parser.add_argument("--snapshot", type=Path, required=True, help="output of spec-manager.py list --json")
    parser.add_argument("--root", type=Path, default=Path("specs"), help="the old status-folder tree")
    parser.add_argument("--dry-run", action="store_true", help="log what would happen, change nothing")
    args = parser.parse_args()
    specs = sorted(json.loads(args.snapshot.read_text()), key=lambda s: int(s["id"]))
    try:
        ids = [int(s["id"]) for s in specs]
        if ids != list(range(1, len(ids) + 1)):
            raise Stop(f"spec ids have gaps ids={ids}")
        login = gh("api", "user", "--jq", ".login").strip()
        if login != LOGIN:
            raise Stop(f"wrong gh account login={login} wanted={LOGIN}; run `gh auth switch`")
        for name, (color, description) in LABELS.items():
            if args.dry_run:
                log("INFO", f"would create label {name}")
            else:
                gh("label", "create", name, "--repo", REPO, "--color", color, "--description", description, "--force")
        create_issues(specs, args.dry_run)
        apply_status(args.root, specs, args.dry_run)
    except Stop as e:
        log("FAIL", str(e))
        return 1
    log("INFO", f"done specs={len(specs)} dry_run={args.dry_run}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
