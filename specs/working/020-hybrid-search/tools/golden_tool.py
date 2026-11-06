#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = []
# ///
"""Sample candidate passages for the golden retrieval set and validate the set.

What it does:
  sample walks the first 200,000 corpus lines in a seeded random order, guesses each
  line's language with a stopword count, drops lines outside the length limits or
  with duplicate text, and prints the next candidates per language. Candidates were
  then read in that order by hand and the first suitable ones became golden entries.
  check validates golden.jsonl: schema, sha256 against the corpus, unique ids and
  lines, and the required composition. Likely quality problems (a question sharing a
  long run of content words with its passage, an answer not found in its passage)
  are WARN lines for a person to read, since names are allowed to repeat.

Output:
  stdout  sample: "<lang>\t<rank>\t<line>\t<passage>" rows.
          check: counts per language, per category and per cross-language pair.
  stderr  Log lines: "YYYY-MM-DD HH:MM LEVEL message key=value ...".

Exit codes:
  0  success
  1  failure (a FAIL line on stderr says why; check logs each problem as ERROR)
  2  invalid command line (printed by argparse)

Side effects:
  None. Only reads the given files.

Examples:
  ./golden_tool.py sample corpus/passages.txt --seed 20 --start 0 --count 80
  ./golden_tool.py check corpus/passages.txt crates/memexvault/tests/fixtures/golden/golden.jsonl

--help output:
[BEGIN HELP]
  $ golden_tool.py --help
  usage: golden_tool.py [-h] [-v] {sample,check} ...

  Sample candidate passages for the golden retrieval set and validate the set.

  positional arguments:
    {sample,check}
      sample        print seeded candidates
      check         validate the golden set

  options:
    -h, --help      show this help message and exit
    -v, --verbose   log DEBUG lines too (default: False)

  $ golden_tool.py sample --help
  usage: golden_tool.py sample [-h] [--seed SEED] [--start START] [--count COUNT]
                               [--language {en,sv,de,es}] [--min-chars MIN_CHARS]
                               [--max-chars MAX_CHARS]
                               passages

  positional arguments:
    passages              corpus file, one passage per line

  options:
    -h, --help            show this help message and exit
    --seed SEED           random seed (default: 20)
    --start START         first rank to print, per language (default: 0)
    --count COUNT         candidates to print, per language (default: 80)
    --language {en,sv,de,es}
                          print only this language (default: None)
    --min-chars MIN_CHARS
                          shortest passage considered (default: 150)
    --max-chars MAX_CHARS
                          longest passage considered (default: 1000)

  $ golden_tool.py check --help
  usage: golden_tool.py check [-h] passages golden

  positional arguments:
    passages    corpus file, one passage per line
    golden      golden.jsonl to validate

  options:
    -h, --help  show this help message and exit
[END HELP]
"""

from __future__ import annotations

import argparse
import hashlib
import json
import logging
import os
import random
import re
import sys
from collections import Counter
from collections.abc import Iterator
from pathlib import Path

log = logging.getLogger("golden_tool")

INDEXED_LINES = 200_000
LANGUAGES = ("en", "sv", "de", "es")
PAIRS = {("sv", "en"), ("en", "sv"), ("de", "en"), ("en", "de"), ("es", "en"), ("en", "es")}
UNANSWERABLE = {"en": 8, "sv": 8, "de": 7, "es": 7}
ANSWERABLE_FIELDS = {"id", "question", "language", "answer_language", "category", "line", "sha256", "answer"}
STOPWORDS = {
    "en": "the of and in to was is for with that as by on from at which were his are it".split(),
    "sv": "och i av en som är att på med för den det till var ett har de om från vid".split(),
    "de": "der die und in den von das mit im ist des dem für wurde auf zu eine ein sich als".split(),
    "es": "de la el en y los del las que un una se por con para fue al su como es".split(),
}
ALL_STOPWORDS = {word for words in STOPWORDS.values() for word in words}
WORD_RE = re.compile(r"[^\W\d_]+|\d+")


# --------------------------------------------------------------------------- logging

logging.addLevelName(logging.WARNING, "WARN")
logging.addLevelName(logging.CRITICAL, "FAIL")
_RESET = "\033[0m"
_KEY_STYLE = "\033[1;97m"
_TIME_STYLE = "\033[2m"
_LEVEL_STYLES = {
    logging.DEBUG: "\033[36m",
    logging.INFO: "\033[32m",
    logging.WARNING: "\033[33m",
    logging.ERROR: "\033[31m",
    logging.CRITICAL: "\033[1;97;41m",
}
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
        return f"{stamp} {level} {message}"


def setup_logging(verbose: bool) -> None:
    handler = logging.StreamHandler(sys.stderr)
    handler.setFormatter(LogFormatter(sys.stderr.isatty() and "NO_COLOR" not in os.environ))
    root = logging.getLogger()
    root.handlers[:] = [handler]
    root.setLevel(logging.DEBUG if verbose else logging.INFO)


def kv(**fields: object) -> str:
    """Render fields as `key=value` pairs; strings with spaces or quotes are JSON-quoted."""
    def fmt(value: object) -> str:
        text = str(value)
        return json.dumps(text, ensure_ascii=False) if not text or any(c.isspace() or c in "\"'=" for c in text) else text
    return " ".join(f"{key}={fmt(value)}" for key, value in fields.items())


class ScriptFailure(Exception):
    """Raised for an expected, explainable failure. main() logs it as FAIL and exits 1."""


# --------------------------------------------------------------------------- pure core


def words(text: str) -> list[str]:
    return WORD_RE.findall(text.lower())


def guess_language(text: str) -> str | None:
    """Return the language whose stopwords are most frequent, or None if there is no clear winner."""
    tokens = words(text)
    scores = {lang: sum(tokens.count(word) for word in STOPWORDS[lang]) for lang in LANGUAGES}
    ranked = sorted(LANGUAGES, key=lambda lang: scores[lang], reverse=True)
    return ranked[0] if scores[ranked[0]] > scores[ranked[1]] else None


def sample_rows(lines: list[str], seed: int, min_chars: int, max_chars: int) -> Iterator[tuple[str, int, int, str]]:
    """Yield (language, rank within language, line number, passage) in seeded random order."""
    counts: Counter[str] = Counter()
    seen: Counter[str] = Counter(lines)
    for number in random.Random(seed).sample(range(len(lines)), len(lines)):
        text = lines[number]
        if not min_chars <= len(text) <= max_chars or seen[text] > 1:
            continue
        lang = guess_language(text)
        if lang is None:
            continue
        yield lang, counts[lang], number, text
        counts[lang] += 1


def longest_shared_run(question: str, passage: str) -> list[str]:
    """Longest run of consecutive question words, minus stopwords and names, also found in the passage."""
    passage_words = " " + " ".join(word for word in words(passage) if word not in ALL_STOPWORDS) + " "
    kept = [word.lower() for word in question.split()[1:]]
    tokens = [word for raw in kept for word in words(raw) if word not in ALL_STOPWORDS]
    best: list[str] = []
    for start in range(len(tokens)):
        for end in range(start + len(best) + 1, len(tokens) + 1):
            if f" {' '.join(tokens[start:end])} " not in passage_words:
                break
            best = tokens[start:end]
    return best


def check_entry(entry: dict[str, object], lines: list[str]) -> tuple[list[str], list[str]]:
    """Return (errors, warnings) for one entry."""
    errors: list[str] = []
    warnings: list[str] = []
    if set(entry) != ANSWERABLE_FIELDS:
        return [f"fields differ from the schema: {sorted(set(entry) ^ ANSWERABLE_FIELDS)}"], []
    question, language, answer_language = entry["question"], entry["language"], entry["answer_language"]
    category, line, digest, answer = entry["category"], entry["line"], entry["sha256"], entry["answer"]
    if not (isinstance(entry["id"], str) and isinstance(question, str) and question.strip()):
        errors.append("id and question must be non-empty strings")
    if language not in LANGUAGES:
        errors.append(f"unknown language {language}")
    if category == "unanswerable":
        if not (line is None and digest is None and answer is None and answer_language is None):
            errors.append("unanswerable entries must have null line, sha256, answer and answer_language")
        return errors, warnings
    if category not in {"natural", "keyword", "identifier", "paraphrase", "compound", "cross_language"}:
        errors.append(f"unknown category {category}")
    if (category == "cross_language") != (language != answer_language) or answer_language not in LANGUAGES:
        errors.append("cross_language must be exactly the entries whose two languages differ")
    if category == "compound" and language not in {"sv", "de"}:
        errors.append("compound is only for sv and de")
    if not (isinstance(answer, str) and answer.strip()):
        errors.append("answer must be a non-empty string")
    if not (isinstance(line, int) and not isinstance(line, bool) and 0 <= line < min(len(lines), INDEXED_LINES)):
        errors.append(f"line out of range: {line}")
        return errors, warnings
    passage = lines[line]
    if hashlib.sha256(passage.encode()).hexdigest() != digest:
        errors.append(f"sha256 does not match line {line}")
    if isinstance(answer, str):
        if len(answer.split()) > 12:
            warnings.append(f"long answer: {answer}")
        if answer.lower() not in passage.lower():
            warnings.append(f"answer not literal in passage: {answer}")
    if isinstance(question, str):
        if category in {"natural", "paraphrase", "cross_language"}:
            run = longest_shared_run(question, passage)
            if len(run) > 3:
                warnings.append(f"shares {len(run)} words in a row: {' '.join(run)}")
        if category == "keyword" and not 2 <= len(question.split()) <= 6:
            errors.append("keyword questions have 2 to 6 words")
    return errors, warnings


def check_composition(entries: list[dict[str, object]]) -> list[str]:
    errors: list[str] = []
    per = Counter((e["language"], e["category"]) for e in entries)
    if len(entries) != 230:
        errors.append(f"expected 230 entries, found {len(entries)}")
    for lang in LANGUAGES:
        same = sum(n for (lang2, cat), n in per.items() if lang2 == lang and cat not in {"cross_language", "unanswerable"})
        if same != 45:
            errors.append(f"{lang}: expected 45 same-language entries, found {same}")
        if per[(lang, "unanswerable")] != UNANSWERABLE[lang]:
            errors.append(f"{lang}: expected {UNANSWERABLE[lang]} unanswerable, found {per[(lang, 'unanswerable')]}")
        if per[(lang, "natural")] < 20:
            errors.append(f"{lang}: expected at least 20 natural, found {per[(lang, 'natural')]}")
        for category in ("keyword", "identifier"):
            if not 6 <= per[(lang, category)] <= 8:
                errors.append(f"{lang}: expected 6 to 8 {category}, found {per[(lang, category)]}")
        if per[(lang, "compound")] != (8 if lang in {"sv", "de"} else 0):
            errors.append(f"{lang}: wrong compound count {per[(lang, 'compound')]}")
    pairs = Counter((e["language"], e["answer_language"]) for e in entries if e["category"] == "cross_language")
    if sum(pairs.values()) != 20:
        errors.append(f"expected 20 cross_language, found {sum(pairs.values())}")
    for pair in sorted(PAIRS):
        if pairs[pair] < 3:
            errors.append(f"cross_language {pair[0]}->{pair[1]}: expected at least 3, found {pairs[pair]}")
    return errors


def check_golden(entries: list[dict[str, object]], lines: list[str]) -> tuple[list[str], list[str]]:
    errors = check_composition(entries)
    warnings: list[str] = []
    for key in ("id", "line"):
        counts = Counter(str(e.get(key)) for e in entries if e.get(key) is not None)
        errors += [f"duplicate {key}: {value}" for value, n in counts.items() if n > 1]
    for entry in entries:
        entry_errors, entry_warnings = check_entry(entry, lines)
        errors += [f"{entry.get('id')}: {text}" for text in entry_errors]
        warnings += [f"{entry.get('id')}: {text}" for text in entry_warnings]
    return errors, warnings


def render_counts(entries: list[dict[str, object]]) -> str:
    def block(title: str, counts: Counter[str]) -> list[str]:
        return [title] + [f"  {key:<16} {counts[key]}" for key in sorted(counts)]
    per_cross = Counter(f"{e['language']}->{e['answer_language']}" for e in entries if e["category"] == "cross_language")
    return "\n".join(
        [f"total {len(entries)}"]
        + block("language", Counter(str(e["language"]) for e in entries))
        + block("category", Counter(str(e["category"]) for e in entries))
        + block("language/category", Counter(f"{e['language']}/{e['category']}" for e in entries))
        + block("cross-language", per_cross)
    )


# --------------------------------------------------------------------------- io edge


def read_passages(path: Path) -> list[str]:
    with path.open(encoding="utf-8", newline="\n") as handle:
        lines = [line.removesuffix("\n") for line in handle]
    log.info("read corpus %s", kv(path=path, lines=len(lines)))
    return lines[:INDEXED_LINES]


def read_golden(path: Path) -> list[dict[str, object]]:
    entries: list[dict[str, object]] = []
    for number, raw in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        try:
            entries.append(json.loads(raw))
        except json.JSONDecodeError as e:
            raise ScriptFailure(f"invalid JSON {kv(line=number, error=str(e))}") from e
    return entries


# --------------------------------------------------------------------------- cli


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=(__doc__ or "").splitlines()[0],
        formatter_class=argparse.ArgumentDefaultsHelpFormatter,
    )
    parser.add_argument("-v", "--verbose", action="store_true", help="log DEBUG lines too")
    commands = parser.add_subparsers(dest="command", required=True)
    sample = commands.add_parser("sample", help="print seeded candidates", formatter_class=argparse.ArgumentDefaultsHelpFormatter)
    sample.add_argument("passages", type=Path, help="corpus file, one passage per line")
    sample.add_argument("--seed", type=int, default=20, help="random seed")
    sample.add_argument("--start", type=int, default=0, help="first rank to print, per language")
    sample.add_argument("--count", type=int, default=80, help="candidates to print, per language")
    sample.add_argument("--language", choices=LANGUAGES, help="print only this language")
    sample.add_argument("--min-chars", type=int, default=150, help="shortest passage considered")
    sample.add_argument("--max-chars", type=int, default=1000, help="longest passage considered")
    check = commands.add_parser("check", help="validate the golden set", formatter_class=argparse.ArgumentDefaultsHelpFormatter)
    check.add_argument("passages", type=Path, help="corpus file, one passage per line")
    check.add_argument("golden", type=Path, help="golden.jsonl to validate")
    return parser.parse_args(argv)


def main() -> int:
    args = parse_args()
    setup_logging(args.verbose)
    log.info("starting %s", kv(command=args.command, passages=args.passages))
    try:
        lines = read_passages(args.passages)
        if args.command == "sample":
            log.info("sampling %s", kv(seed=args.seed, start=args.start, count=args.count))
            for lang, rank, number, text in sample_rows(lines, args.seed, args.min_chars, args.max_chars):
                if (args.language in (None, lang)) and args.start <= rank < args.start + args.count:
                    print(f"{lang}\t{rank}\t{number}\t{text}")
            return 0
        entries = read_golden(args.golden)
        errors, warnings = check_golden(entries, lines)
        print(render_counts(entries))
        for text in warnings:
            log.warning("%s", text)
        for text in errors:
            log.error("%s", text)
        if errors:
            raise ScriptFailure(f"golden set is invalid {kv(errors=len(errors), warnings=len(warnings))}")
        log.info("golden set is valid %s", kv(entries=len(entries), warnings=len(warnings)))
        return 0
    except ScriptFailure as e:
        log.critical("%s", e)
        return 1


if __name__ == "__main__":
    sys.exit(main())
