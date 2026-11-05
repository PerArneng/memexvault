#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12"
# dependencies = [
#     "pydantic>=2.7",
#     "pymupdf4llm==2.1",
# ]
# ///
"""Convert every born-digital PDF in a directory to clean Markdown files.

What it does:
  Runs pymupdf4llm (layout mode, OCR off) on each *.pdf in INPUT_DIR, then repairs
  what it measurably gets wrong, using the PDF itself as the reference: lines repeated
  across pages (running headers and footers), words joined at line breaks (including
  lost hyphens), dropped link targets, paragraphs and code blocks split at page breaks, false
  list items, <br> in table cells, and ASCII diagrams, which are rebuilt from character
  coordinates into a fenced code block. Each output starts with YAML front matter
  naming its source. The converter's own header=False is not used: it also drops body
  text at page edges.

Output:
  stdout  The path of each file written (or that would be written with --dry-run).
  stderr  Log lines: "YYYY-MM-DD HH:MM LEVEL message key=value ...".
          A WARN line lists any link that could not be placed in the text.

Exit codes:
  0  success
  1  failure (a FAIL line on stderr says why)
  2  invalid command line (printed by argparse)

Side effects:
  Writes OUTPUT_DIR/<kebab-case-pdf-name>.md, creating OUTPUT_DIR and overwriting
  existing files. Reads the PDFs only. Nothing is written with --dry-run.

Examples:
  ./convert.py ~/Downloads/OptimalAgentKnowledgeBase research
  ./convert.py -v --dry-run ~/Downloads/OptimalAgentKnowledgeBase research

--help output:
[BEGIN HELP]
  $ convert.py --help
  usage: convert.py [-h] [-v] [--dry-run] INPUT_DIR OUTPUT_DIR

  Convert every born-digital PDF in a directory to clean Markdown files.

  positional arguments:
    INPUT_DIR      directory holding the *.pdf files
    OUTPUT_DIR     directory to write the Markdown files to

  options:
    -h, --help     show this help message and exit
    -v, --verbose  log DEBUG lines too (default: False)
    --dry-run      convert and log, but write nothing (default: False)
[END HELP]
"""

# PyMuPDF ships type hints, but most of its functions are unannotated.
# mypy: disable-error-code="no-untyped-call"

from __future__ import annotations

import argparse
import hashlib
import json
import logging
import os
import re
import statistics
import sys
import time
from collections import Counter
from collections.abc import Callable
from pathlib import Path

import pymupdf
import pymupdf.layout  # noqa: F401  (importing it enables layout mode)
import pymupdf4llm  # type: ignore[import-untyped]
from pydantic import BaseModel, ConfigDict

log = logging.getLogger("convert")


# --------------------------------------------------------------------------- logging

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


class PdfLink(BaseModel):
    model_config = ConfigDict(frozen=True)

    label: str
    uri: str
    page: int  # 0-based
    before: list[str]  # the PDF words just before the label, which tell repeated labels apart
    after: list[str]


class WordPair(BaseModel):
    """Two words that follow each other in the PDF."""

    model_config = ConfigDict(frozen=True)

    first: str
    second: str
    across_lines: bool


class PdfFacts(BaseModel):
    """What the Markdown is repaired against, read from the PDF itself."""

    model_config = ConfigDict(frozen=True)

    page_count: int
    words: frozenset[str]
    pairs: list[WordPair]
    links: list[PdfLink]


class PlacedChar(BaseModel):
    """A character of a diagram: its row (top to bottom) and left edge in points."""

    model_config = ConfigDict(frozen=True)

    char: str
    row: int
    x: float
    width: float


# --------------------------------------------------------------------------- pure core

_PICTURE_RE = re.compile(r"<!-- Start of picture text -->.*?(?=^#{1,6} )", re.DOTALL | re.MULTILINE)
_HEADING_RE = re.compile(r"^#{1,6} (.+?)\s*$", re.MULTILINE)
_DIAGRAM_PLACEHOLDER = "@@DIAGRAM-{}@@"
_LINK_RE = r"\[[^\]]+\]\([^)]+\)"
_ARROWS = "→←↔"
_PAGE_SEPARATOR_RE = re.compile(r"^--- end of page\.page_number=\d+ ---[ \t]*$", re.MULTILINE)
_CONTEXT_WORDS = 3


def kebab_case(stem: str) -> str:
    return re.sub(r"[^a-z0-9]+", "-", stem.lower()).strip("-")


def plain_heading(heading: str) -> str:
    return heading.replace("*", "").strip()


def extract_diagrams(md: str, render: Callable[[str, str], str]) -> tuple[str, list[str]]:
    """Swap each garbled picture block for a placeholder and return the rebuilt diagrams.

    `render` gets the headings around the block, which locate the diagram in the PDF.
    """
    diagrams: list[str] = []

    def replace(match: re.Match[str]) -> str:
        before = _HEADING_RE.findall(md, 0, match.start())
        after = _HEADING_RE.search(md, match.end())
        if not before or after is None:
            raise ScriptFailure(f"picture block without surrounding headings {kv(offset=match.start())}")
        diagrams.append(render(plain_heading(before[-1]), plain_heading(after.group(1))))
        # A diagram can cross a page, so keep any page separators the block swallowed.
        separators = _PAGE_SEPARATOR_RE.findall(match.group(0))
        return "\n\n".join([_DIAGRAM_PLACEHOLDER.format(len(diagrams) - 1), *separators]) + "\n\n"

    return _PICTURE_RE.sub(replace, md), diagrams


def render_monospace(chars: list[PlacedChar]) -> str:
    """Lay characters out on a text grid by position, so the diagram keeps its shape."""
    if not chars:
        raise ScriptFailure("diagram region holds no text")
    cell = statistics.median(c.width for c in chars)
    left = min(c.x for c in chars)
    rows: dict[int, dict[int, str]] = {}
    ends: dict[int, tuple[float, int]] = {}  # per row: right edge and column of the last character
    for c in sorted(chars, key=lambda c: (c.row, c.x)):
        row = rows.setdefault(c.row, {})
        previous_end, previous_column = ends.get(c.row, (-cell, -1))
        # Characters that touch stay adjacent: box-drawing glyphs are wider than a text cell,
        # and placing them by position alone would leave gaps in their lines.
        touching = abs(c.x - previous_end) < cell / 2
        column = previous_column + 1 if touching else max(round((c.x - left) / cell), previous_column + 1)
        row[column] = c.char
        ends[c.row] = (c.x + c.width, column)
    lines = ["".join(row.get(i, " ") for i in range(max(row) + 1)) for _, row in sorted(rows.items())]
    return "```text\n" + "\n".join(lines) + "\n```"


def _joined_word_re(joined: str, first: str, second: str) -> re.Pattern[str]:
    """Match `joined` as a whole token. Only an arrow may touch its neighbour: x→y holds two pairs."""
    start = "" if first[0] in _ARROWS else r"(?<![\w-])"
    end = "" if second[-1] in _ARROWS else r"(?![\w-])"
    return re.compile(start + re.escape(joined) + end)


def repair_joined_words(md: str, facts: PdfFacts) -> tuple[str, int]:
    """Split words the converter glued together, and restore hyphens it dropped at line ends.

    A token absent from the PDF can only come from the conversion, so a pair is repaired
    only when its joined form is not a word of the PDF. A hyphen at a line end counts as
    real: these PDFs have no soft hyphens, and their renderers don't hyphenate. A wrap after
    a slash, or before a hyphen, slash or dot, hides no space: pgvector/ + pgvectorscale.
    """
    repairs = 0
    for pair in facts.pairs:
        if pair.across_lines and pair.first.endswith("-") and len(pair.first) > 1:
            joined, fixed = pair.first[:-1] + pair.second, pair.first + pair.second
        elif pair.across_lines and (pair.first.endswith("/") or pair.second.startswith(("-", "/", "."))):
            continue
        else:
            joined, fixed = pair.first + pair.second, f"{pair.first} {pair.second}"
        if joined in facts.words:
            continue
        md, count = _joined_word_re(joined, pair.first, pair.second).subn(lambda _: fixed, md)
        if count:
            log.debug("repaired joined word %s", kv(joined=joined, fixed=fixed, count=count))
        repairs += count
    return md, repairs


def _link(label: str, uri: str) -> str:
    return f"[{label}]({uri.replace('(', '%28').replace(')', '%29')})"


def split_pages(md: str, page_count: int) -> list[str]:
    pages = _PAGE_SEPARATOR_RE.split(md)
    if not pages[-1].strip():
        pages.pop()
    if len(pages) != page_count:
        raise ScriptFailure(f"page separators don't match the PDF {kv(found=len(pages), expected=page_count)}")
    return pages


def strip_running_lines(pages: list[str]) -> tuple[list[str], list[str]]:
    """Remove running headers, footers and page numbers: lines repeated on at least half of the pages."""
    seen: Counter[str] = Counter()
    for page in pages:
        seen.update({_running_key(line) for line in page.splitlines() if _is_text_line(line)})
    threshold = max(3, len(pages) / 2)
    running = {key for key, count in seen.items() if count >= threshold}
    cleaned = [
        "\n".join(
            line for line in page.splitlines() if not (_is_text_line(line) and _running_key(line) in running)
        )
        for page in pages
    ]
    return cleaned, sorted(running)


def _is_text_line(line: str) -> bool:
    return any(c.isalpha() for c in line) and not line.lstrip().startswith(("|", "#", "```"))


def _running_key(line: str) -> str:
    return re.sub(r"^Page \d+$", "Page N", line.strip())


def _context_tokens(text: str) -> list[str]:
    text = re.sub(rf"\[([^\]]+)\]\([^)]+\)", r"\1", text).replace("<br>", " ")
    return [t for t in (re.sub(r"^\W+|\W+$", "", w).lower() for w in re.split(r"[\s|*`]+", text)) if t]


def _context_score(page: str, start: int, end: int, link: PdfLink) -> int:
    """Count the PDF words around a link that also surround this occurrence of its label."""
    before = _context_tokens(page[:start])[-_CONTEXT_WORDS:]
    after = _context_tokens(page[end:])[:_CONTEXT_WORDS]
    wanted_before = _context_tokens(" ".join(link.before))[-_CONTEXT_WORDS:]
    wanted_after = _context_tokens(" ".join(link.after))[:_CONTEXT_WORDS]
    return sum(a == b for a, b in zip(reversed(before), reversed(wanted_before))) + sum(
        a == b for a, b in zip(after, wanted_after)
    )


def restore_links(pages: list[str], links: list[PdfLink]) -> tuple[list[str], list[PdfLink]]:
    """Put each link target back on its label. Returns the links that could not be placed.

    A label such as `arxiv` repeats with different targets, so each link goes on the
    occurrence, on its own page, whose surrounding words best match the PDF.
    """
    pages = [re.sub(r"</?u>", "", page) for page in pages]
    unplaced: list[PdfLink] = []
    for link in links:
        page = pages[link.page]
        label_re = r"(?:\s|<br>)+".join(re.escape(word) for word in link.label.split())
        # Matching whole links first skips over them, so a label is never placed inside a URL.
        pattern = re.compile(rf"{_LINK_RE}|(?<![\w\[/.-])(?P<label>{label_re})(?![\w\]-]|\.\w)")
        spots = [m for m in pattern.finditer(page) if m.group("label")]
        if not spots:
            unplaced.append(link)
            continue
        best = max(spots, key=lambda m: _context_score(page, m.start(), m.end(), link))
        log.debug(
            "placed link %s",
            kv(label=link.label, uri=link.uri, page=link.page, candidates=len(spots),
               score=_context_score(page, best.start(), best.end(), link)),
        )
        pages[link.page] = page[: best.start()] + _link(link.label, link.uri) + page[best.end() :]
    return pages, unplaced


def escape_table_pipes(md: str, words: frozenset[str]) -> str:
    """Escape pipes that belong to the text of a table cell. The PDF has no cell delimiters,
    so any pipe inside one of its words is content: view=page|claims|provenance.
    """
    piped = sorted((w for w in words if "|" in w and len(w) > 1), key=len, reverse=True)
    if not piped:
        return md
    pattern = re.compile("|".join(re.escape(w) for w in piped))
    lines = [
        pattern.sub(lambda m: m.group(0).replace("|", r"\|"), line) if line.startswith("|") else line
        for line in md.split("\n")
    ]
    return "\n".join(lines)


def flatten_table_breaks(md: str) -> str:
    """Remove <br> from table cells, so phrases that wrapped in the PDF can be searched."""
    md = re.sub(r"(?<=[a-z])<br>(?=[b-z]\b)", "", md)  # a word broken mid-way: Readabl<br>e
    md = re.sub(r"(?<=\S[-/–])<br>", "", md)  # a break inside a compound: Lifecycle-<br>managed, ~4–<br>10M
    return md.replace("<br>", " ")


def tidy_lines(md: str) -> str:
    md = md.replace("​", "")
    md = re.sub(r"^(#{1,6}) \*\*(.+?)\*\*\s*$", r"\1 \2", md, flags=re.MULTILINE)
    md = re.sub(r"^\s*[•●○▪◦]\s+", "- ", md, flags=re.MULTILINE)
    md = re.sub(r"\*\* ([:;,.])", r"**\1", md)
    md = re.sub(r"^(#{1,6} .+)\n(?=\S)", r"\1\n\n", md, flags=re.MULTILINE)
    md = re.sub(r"[ \t]+$", "", md, flags=re.MULTILINE)
    return re.sub(r"\n{3,}", "\n\n", md).strip() + "\n"


def _is_structural(block: str) -> bool:
    return block.startswith(("#", "|", "```", "@@"))


def merge_split_blocks(md: str) -> tuple[str, int]:
    """Rejoin paragraphs split at page breaks, and bullets the converter split from a wrapped line."""
    merged: list[str] = []
    joins = 0
    for block in md.split("\n\n"):
        previous = merged[-1] if merged else ""
        if previous and not _is_structural(previous) and not _is_structural(block):
            continues_paragraph = block[0].islower() and previous[-1] not in ".:!?"
            continues_bullet = (
                previous.startswith("- ")
                and block.startswith("- ")
                and not block.startswith("- **")
                and previous[-1] in ",;"
            )
            if continues_paragraph or continues_bullet:
                merged[-1] = f"{previous} {block.removeprefix('- ')}"
                joins += 1
                continue
        merged.append(block)
    return "\n\n".join(merged), joins


def split_reference_list(md: str, heading: str = "Works cited") -> str:
    """Give each numbered reference its own line. The converter runs them together in paragraphs."""
    match = re.search(rf"^#{{1,6}} {heading}$", md, re.MULTILINE)
    if match is None:
        return md
    next_heading = re.compile(r"^#{1,6} ", re.MULTILINE).search(md, match.end())
    end = next_heading.start() if next_heading else len(md)
    text = " ".join(md[match.end() : end].split())
    entries: list[str] = []
    number = 2
    start = 0
    while (found := text.find(f" {number}. ", start)) != -1:
        entries.append(text[start:found])
        start = found + 1
        number += 1
    entries.append(text[start:])
    log.info("split reference list %s", kv(heading=heading, entries=len(entries)))
    return md[: match.end()] + "\n\n" + "\n".join(entries) + "\n\n" + md[end:]


def listify_link_paragraphs(md: str) -> str:
    """Turn paragraphs holding nothing but links (reference lists) into list items."""
    only_links = re.compile(rf"^(?:{_LINK_RE}\s*)+$")
    blocks = [
        "\n".join(f"- {link}" for link in re.findall(_LINK_RE, block)) if only_links.match(block) else block
        for block in md.split("\n\n")
    ]
    return "\n\n".join(blocks)


def front_matter(source: str, sha256: str, pages: int, converter: str) -> str:
    # json.dumps gives a double-quoted string, which is valid YAML.
    return (
        f"---\nsource: {json.dumps(source, ensure_ascii=False)}\nsha256: {sha256}\n"
        f"pages: {pages}\nconverter: {json.dumps(converter)}\n---\n\n"
    )


def clean_markdown(raw: str, facts: PdfFacts, render_diagram: Callable[[str, str], str]) -> tuple[str, list[PdfLink]]:
    """Apply every repair. Returns the Markdown and any links that could not be placed."""
    md, diagrams = extract_diagrams(raw, render_diagram)
    md, repairs = repair_joined_words(md, facts)
    md = escape_table_pipes(md, facts.words)
    pages, running = strip_running_lines(split_pages(md, facts.page_count))
    pages, unplaced = restore_links(pages, facts.links)
    md = flatten_table_breaks("\n\n".join(pages))
    md = tidy_lines(md)
    md, joins = merge_split_blocks(md)
    md, fence_joins = re.subn(r"^```\n\n```[^\n]*\n", "", md, flags=re.MULTILINE)  # a code block split at a page break
    md = split_reference_list(md)
    md = listify_link_paragraphs(md)
    for i, diagram in enumerate(diagrams):
        md = md.replace(_DIAGRAM_PLACEHOLDER.format(i), diagram)
    log.info(
        "cleaned %s",
        kv(running_lines=running, word_repairs=repairs, block_joins=joins, fence_joins=fence_joins, diagrams=len(diagrams),
           links=len(facts.links) - len(unplaced), links_unplaced=len(unplaced)),
    )
    return md.rstrip() + "\n", unplaced


# --------------------------------------------------------------------------- io edge


def read_facts(doc: pymupdf.Document) -> PdfFacts:
    words: set[str] = set()
    pairs: list[WordPair] = []
    links: list[PdfLink] = []
    for number in range(doc.page_count):
        page = doc[number]
        page_words = page.get_text("words")
        tokens = [w[4] for w in page_words]
        words.update(w[4] for w in page_words)
        for a, b in zip(page_words, page_words[1:]):
            pairs.append(WordPair(first=a[4], second=b[4], across_lines=abs(a[3] - b[3]) > (a[3] - a[1]) / 2))
        for link in page.get_links():
            if link.get("uri"):
                rect = link["from"]
                inside = [i for i, w in enumerate(page_words) if pymupdf.Rect(w[:4]).intersects(rect)]
                first, last = (inside[0], inside[-1]) if inside else (0, -1)
                links.append(
                    PdfLink(
                        label=" ".join(page.get_textbox(rect).split()),
                        uri=link["uri"],
                        page=page.number,
                        before=tokens[max(0, first - 3) : first],
                        after=tokens[last + 1 : last + 4],
                    )
                )
    return PdfFacts(page_count=doc.page_count, words=frozenset(words), pairs=pairs, links=links)


def _find_heading(doc: pymupdf.Document, heading: str, first_page: int) -> tuple[int, pymupdf.Rect]:
    for number in range(first_page, doc.page_count):
        rects = doc[number].search_for(heading)
        if rects:
            return number, rects[0]
    raise ScriptFailure(f"heading not found in the PDF {kv(heading=heading)}")


def read_region_chars(doc: pymupdf.Document, before: str, after: str) -> list[PlacedChar]:
    """Read the non-blank characters between two headings, across pages if needed."""
    start_page, start_rect = _find_heading(doc, before, 0)
    end_page, end_rect = _find_heading(doc, after, start_page)
    chars: list[PlacedChar] = []
    row = -1
    for number in range(start_page, end_page + 1):
        top = start_rect.y1 if number == start_page else 0.0
        bottom = end_rect.y0 if number == end_page else float("inf")
        baseline = -100.0
        spans = [
            span
            for block in doc[number].get_text("rawdict")["blocks"]
            for line in block.get("lines", [])
            for span in line["spans"]
        ]
        placed = sorted(
            (ch for span in spans for ch in span["chars"] if top < ch["origin"][1] < bottom),
            key=lambda ch: ch["origin"][1],
        )
        for ch in placed:
            if ch["origin"][1] - baseline > 4:  # small symbol fonts sit slightly off the baseline
                row += 1
                baseline = ch["origin"][1]
            if not ch["c"].isspace():
                x0, _, x1, _ = ch["bbox"]
                chars.append(PlacedChar(char=ch["c"], row=row, x=x0, width=x1 - x0))
    return chars


def convert_pdf(path: Path) -> tuple[str, PdfFacts, Callable[[str, str], str], str]:
    doc = pymupdf.open(path)
    raw: str = pymupdf4llm.to_markdown(doc, use_ocr=False, page_separators=True)
    log.info("converted %s", kv(pdf=path.name, pages=doc.page_count, raw_chars=len(raw)))

    def render(before: str, after: str) -> str:
        log.info("rebuilding diagram %s", kv(before=before, after=after))
        return render_monospace(read_region_chars(doc, before, after))

    return raw, read_facts(doc), render, hashlib.sha256(path.read_bytes()).hexdigest()


# --------------------------------------------------------------------------- cli


def parse_args(argv: list[str] | None = None) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=(__doc__ or "").splitlines()[0],
        formatter_class=argparse.ArgumentDefaultsHelpFormatter,
    )
    parser.add_argument("input_dir", type=Path, metavar="INPUT_DIR", help="directory holding the *.pdf files")
    parser.add_argument("output_dir", type=Path, metavar="OUTPUT_DIR", help="directory to write the Markdown files to")
    parser.add_argument("-v", "--verbose", action="store_true", help="log DEBUG lines too")
    parser.add_argument("--dry-run", action="store_true", help="convert and log, but write nothing")
    return parser.parse_args(argv)


def main() -> int:
    args = parse_args()
    setup_logging(args.verbose)
    started = time.monotonic()
    log.info("starting %s", kv(input_dir=args.input_dir, output_dir=args.output_dir, dry_run=args.dry_run))
    pdfs = sorted(args.input_dir.glob("*.pdf"))
    converter = f"pymupdf4llm {pymupdf4llm.__version__}"
    try:
        if not pdfs:
            raise ScriptFailure(f"no PDF files found {kv(input_dir=args.input_dir)}; check the path")
        for pdf in pdfs:
            raw, facts, render, sha256 = convert_pdf(pdf)
            md, unplaced = clean_markdown(raw, facts, render)
            for link in unplaced:
                log.warning("link not placed in the text %s", kv(pdf=pdf.name, label=link.label, uri=link.uri))
            target = args.output_dir / f"{kebab_case(pdf.stem)}.md"
            if not args.dry_run:
                args.output_dir.mkdir(parents=True, exist_ok=True)
                target.write_text(front_matter(pdf.name, sha256, facts.page_count, converter) + md, encoding="utf-8")
            print(target)
    except ScriptFailure as e:
        log.critical("%s", e)
        return 1
    log.info("done %s", kv(files=len(pdfs), elapsed_s=time.monotonic() - started))
    return 0


if __name__ == "__main__":
    sys.exit(main())
