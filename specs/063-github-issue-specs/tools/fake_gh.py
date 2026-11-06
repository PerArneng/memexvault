#!/usr/bin/env python3
"""A stand-in for the gh command, for test_gh_spec_manager.py. Keeps issues in the JSON file $FAKE_GH_STATE.

It implements only the calls gh-spec-manager.py makes, and fails on anything else, so a test can't pass
by a call being silently ignored. Every call is appended to the state's "calls" list.
"""

from __future__ import annotations

import json
import os
import sys
from pathlib import Path

STATE = Path(os.environ["FAKE_GH_STATE"])


def flags(args: list[str]) -> tuple[list[str], dict[str, list[str]]]:
    positional: list[str] = []
    named: dict[str, list[str]] = {}
    i = 0
    while i < len(args):
        if args[i].startswith("--"):
            named.setdefault(args[i], []).append(args[i + 1])
            i += 2
        else:
            positional.append(args[i])
            i += 1
    return positional, named


def fail(message: str, code: int = 1) -> int:
    print(message, file=sys.stderr)
    return code


def view(issue: dict) -> dict:
    return {**issue, "labels": [{"name": label} for label in issue["labels"]]}


def main() -> int:
    argv = sys.argv[1:]
    state = json.loads(STATE.read_text())
    state["calls"].append(argv)
    pos, named = flags(argv)
    issues = state["issues"]
    code = run(pos, named, state, issues)
    STATE.write_text(json.dumps(state))
    return code


def run(pos: list[str], named: dict[str, list[str]], state: dict, issues: dict) -> int:
    if state.get("fail_on") and state["fail_on"] in " ".join(pos):
        return fail("HTTP 502: fake failure")
    if pos[:2] == ["repo", "view"]:
        print(json.dumps({"nameWithOwner": "o/r", "viewerPermission": state.get("permission", "ADMIN")}))
        return 0
    if pos[:2] == ["issue", "list"]:
        scheduled = state.get("close_after_lists")
        if scheduled:  # lets a test finish a parent wave while a poller is waiting
            scheduled["lists"] -= 1
            if scheduled["lists"] <= 0:
                for n in scheduled["numbers"]:
                    issues[str(n)].update(state="CLOSED", stateReason="COMPLETED")
                state["close_after_lists"] = None
        label = named.get("--label", [None])[0]
        print(json.dumps([view(i) for i in issues.values() if label is None or label in i["labels"]]))
        return 0
    if pos[:2] == ["issue", "create"]:
        n = state["next"]
        state["next"] = n + 1
        issues[str(n)] = {"number": n, "title": named["--title"][0], "body": named["--body"][0], "state": "OPEN",
                          "stateReason": "", "url": f"https://github.com/o/r/issues/{n}",
                          "labels": list(named["--label"]), "comments": []}
        print(f"https://github.com/o/r/issues/{n}")
        return 0
    if pos[0] == "issue" and len(pos) == 3:
        issue = issues.get(pos[2])
        if issue is None:
            return fail(f"GraphQL: Could not resolve to an issue or pull request with the number of {pos[2]}.")
        if pos[1] == "view":
            print(json.dumps(view(issue)))
        elif pos[1] == "edit":
            issue["title"] = named.get("--title", [issue["title"]])[0]
            issue["body"] = named.get("--body", [issue["body"]])[0]
            issue["labels"] = [x for x in issue["labels"] if x not in named.get("--remove-label", [])]
            issue["labels"] += named.get("--add-label", [])
        elif pos[1] == "comment":
            issue["comments"].append(named["--body"][0])
        elif pos[1] == "reopen":
            issue.update(state="OPEN", stateReason="REOPENED")
        elif pos[1] == "close":
            reason = named["--reason"][0]
            issue.update(state="CLOSED", stateReason={"completed": "COMPLETED", "not planned": "NOT_PLANNED"}[reason])
            issue["comments"] += named.get("--comment", [])
        else:
            return fail(f"fake gh: unsupported call {pos}", 99)
        return 0
    return fail(f"fake gh: unsupported call {pos}", 99)


if __name__ == "__main__":
    sys.exit(main())
