#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = []
# ///
"""Samples pairs of corpus passages that name the same thing, for writing two-hop questions, and checks the finished fixture.

The corpus is untitled paragraphs, so a capitalized phrase of two or more words found in only a few passages stands in for
a shared topic. Each pair is two passages naming one such phrase. Output is for a question writer's eyes and goes to a
file outside the repo, because the passages are Wikipedia text and aren't committed.

usage: relational_pairs.py [-h] {sample,check} ...

  sample --corpus FILE --out FILE [--seed N] [--count N] [--total N]
  check  --corpus FILE --fixture FILE [--total N]

Log lines look like: 2021-01-12 15:56 INFO message key=value
"""
import argparse
import hashlib
import json
import random
import re
import sys
from collections import defaultdict
from datetime import datetime
from pathlib import Path

PHRASE = re.compile(r"\b[A-Z][\w'-]+(?: [A-Z][\w'-]+)+\b")
REQUIRED = {"id", "question", "answer", "language", "category", "from_line", "from_sha256", "line", "sha256"}
MAX_TEXT_CHARS = 300


def log(level: str, message: str, **fields: object) -> None:
    """Writes one log line to stderr."""
    extras = " ".join(f"{k}={v}" for k, v in fields.items())
    print(f"{datetime.now():%Y-%m-%d %H:%M} {level} {message} {extras}".rstrip(), file=sys.stderr)


def content_hash(text: str) -> str:
    """The same hash golden.jsonl uses for a passage."""
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def read_corpus(path: Path, total: int) -> list[str]:
    """The first `total` passages, one per line."""
    with path.open(encoding="utf-8") as f:
        return [line.rstrip("\n") for _, line in zip(range(total), f)]


def sample_pairs(passages: list[str], seed: int, count: int) -> list[dict[str, object]]:
    """Pure. Up to `count` pairs, each from a different phrase, drawn with `seed`."""
    where: dict[str, set[int]] = defaultdict(set)
    for i, text in enumerate(passages):
        for phrase in set(PHRASE.findall(text)):
            where[phrase].add(i)
    usable = sorted(p for p, lines in where.items() if 2 <= len(lines) <= 5)
    rng = random.Random(seed)
    rng.shuffle(usable)
    pairs: list[dict[str, object]] = []
    used: set[int] = set()
    for phrase in usable:
        lines = sorted(where[phrase] - used)
        if len(lines) < 2:
            continue
        a, b = rng.sample(lines, 2)
        used.update((a, b))
        pairs.append({"phrase": phrase, "a_line": a, "a_sha256": content_hash(passages[a]), "a": passages[a], "b_line": b, "b_sha256": content_hash(passages[b]), "b": passages[b]})
        if len(pairs) == count:
            break
    return pairs


def check_fixture(rows: list[dict[str, object]], passages: list[str]) -> list[str]:
    """Pure. Every problem found, in plain words."""
    problems: list[str] = []
    seen: set[object] = set()
    for row in rows:
        rid = row.get("id")
        missing = REQUIRED - row.keys()
        if missing:
            problems.append(f"{rid}: missing {sorted(missing)}")
            continue
        if rid in seen:
            problems.append(f"{rid}: the id appears twice")
        seen.add(rid)
        if row["category"] != "relational":
            problems.append(f"{rid}: the category must be relational")
        for line_key, hash_key in (("from_line", "from_sha256"), ("line", "sha256")):
            line = row[line_key]
            if not isinstance(line, int) or not 0 <= line < len(passages):
                problems.append(f"{rid}: {line_key} is out of range")
            elif content_hash(passages[line]) != row[hash_key]:
                problems.append(f"{rid}: {hash_key} doesn't match line {line}")
        for key in ("question", "answer"):
            if len(str(row[key])) > MAX_TEXT_CHARS:
                problems.append(f"{rid}: {key} is over {MAX_TEXT_CHARS} characters")
        line = row["line"]
        if isinstance(line, int) and 0 <= line < len(passages) and str(row["answer"]) not in passages[line]:
            problems.append(f"{rid}: the answer isn't verbatim in passage B")
    return problems


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    commands = parser.add_subparsers(dest="command", required=True)
    sample = commands.add_parser("sample")
    sample.add_argument("--corpus", type=Path, required=True)
    sample.add_argument("--out", type=Path, required=True)
    sample.add_argument("--seed", type=int, default=22)
    sample.add_argument("--count", type=int, default=150)
    sample.add_argument("--total", type=int, default=200_000)
    check = commands.add_parser("check")
    check.add_argument("--corpus", type=Path, required=True)
    check.add_argument("--fixture", type=Path, required=True)
    check.add_argument("--total", type=int, default=200_000)
    args = parser.parse_args()
    passages = read_corpus(args.corpus, args.total)
    if args.command == "sample":
        pairs = sample_pairs(passages, args.seed, args.count)
        args.out.write_text("".join(json.dumps(p, ensure_ascii=False) + "\n" for p in pairs), encoding="utf-8")
        log("INFO", "sampled", pairs=len(pairs), out=args.out)
        return 0
    rows = [json.loads(line) for line in args.fixture.read_text(encoding="utf-8").splitlines() if line.strip()]
    problems = check_fixture(rows, passages)
    for problem in problems:
        log("ERROR", problem)
    log("INFO" if not problems else "FAIL", "checked", rows=len(rows), problems=len(problems))
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
