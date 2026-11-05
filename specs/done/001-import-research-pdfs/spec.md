---
id: 001
name: import-research-pdfs
description: Convert the agent knowledge base research PDFs to Markdown in research/
---

# 001 import-research-pdfs

## Goal
Put the three research reports from `/Users/perarneng/Downloads/OptimalAgentKnowledgeBase` into `research/` as clean
Markdown, so agents and people can read, search and cite them from the repo. A conversion must keep all the content
and add no page noise.

## Background
The user asked for these PDFs to be imported using PyMuPDF or a similar CLI, with the best conversion quality
available.

Measured on 2026-11-05 with poppler. All three are born-digital: no images, no encryption, no OCR needed.

| PDF | Producer | Pages | Words | What makes it hard |
|---|---|---|---|---|
| `Agent-Native Knowledge System Architecture.pdf` | Google Docs (tagged) | 14 | 5175 | Bullets carry a zero-width space (`●` + U+200B) |
| `Optimal_Agent_Native_Knowledge_System_2026.pdf` | ReportLab | 8 | 2366 | Running header and footer on each page, table of contents |
| `The Agent-Native Knowledge System_ A Reference Architecture for.pdf` | Chromium | 25 | 6021 | Multi-column tables, 16 fonts, inline citation labels (`arxiv`) |

Licensing: `pymupdf4llm` and `pymupdf` are AGPL-3.0 or commercial; the repo is Apache-2.0. Running them as a
dev-time converter through `uv` adds nothing to the repo's dependencies or distribution, and the product must not
import them.

Publication: the repo is public (`PerArneng/memexvault`), and the Optimal report has a "Confidential Research
Synthesis" footer on all 8 pages. Removing the footer during cleanup doesn't settle whether the content can be
published. **Get the user's explicit OK before committing `research/`.**

## Approach
- **Converter:** `pymupdf4llm` 2.1 (pinned), which pulls in `pymupdf_layout` for layout analysis. It is built for
  LLM-ready Markdown: headings, tables, links, reading order.
- **Script:** `convert.py` in this spec's folder, a uv single-file script following the
  `uv-based-python-singlefile-script` skill. It takes the input and output directories as arguments. Living in the
  spec folder means this one-off import doesn't decide a layout for product code.
- **Output:** one file per PDF in `research/`, with a kebab-case name derived from the PDF's file name.
- **Front matter:** source file name, SHA-256, page count, converter and version.
- **Cleanup, only where the raw output needs it:** remove running headers, footers and page numbers; normalize
  bullets and zero-width characters. Headings, tables, links and citation labels stay as they are.
- **Fallback:** if a file fails Verification and cleanup can't fix it, try Docling on that file and record the
  comparison in Result.
- **Docs:** add a line about `research/` to `AGENTS.md`.

## Verification
Record each measured value in Result.

1. Three Markdown files exist in `research/`, each with complete front matter.
2. Word count for each file is at least 97% of the baseline in Background, excluding front matter and removed
   running headers and footers.
3. `grep -c "Confidential Research Synthesis"` returns 0 on the Optimal report, and no page-number lines remain.
4. No U+200B remains in any file.
5. The Reference Architecture tables are valid pipe tables. Check that the `Object / Purpose / Mutable?` and
   `Pattern / Consistency / Verdict` tables have intact rows and columns.
6. Spot-check figures against the PDF: `471.57`, `41.47` and `92%` are present and in the right sentences.
7. Read one page per PDF beside its source, side by side, for reading order and heading levels.

## Out of scope
- Copying the source PDFs into the repo.
- OCR and image extraction, since none of the PDFs has images.
- Summarizing, editing or fact-checking the reports.
- A reusable ingestion pipeline or any decision about product stack or storage format.

## Result
**Kept.** `research/` holds all three reports as Markdown, produced by `convert.py` in this folder. Docling wasn't
needed. **Nothing is committed: `research/` still needs the user's OK** (see Background).

```bash
specs/done/001-import-research-pdfs/convert.py ~/Downloads/OptimalAgentKnowledgeBase research
```

| Output in `research/` | Pages | Words, PDF → Markdown | Links | Table rows | Headings |
|---|---|---|---|---|---|
| `agent-native-knowledge-system-architecture.md` | 14 | 5238 → 5242 (100.1%) | 0 of 0 | 5 | 41 |
| `optimal-agent-native-knowledge-system-2026.md` | 8 | 2250 → 2255 (100.2%) | 15 of 15 | 0 | 22 |
| `the-agent-native-knowledge-system-a-reference-architecture-for.md` | 25 | 6172 → 6148 (99.6%) | 65 of 65 | 144 | 29 |

Verification, as measured on 2026-11-05:

1. Passed. Three files, each with complete front matter.
2. Passed, with a changed method. The plain `wc -w` planned in this spec gives 95.9% for the Reference Architecture,
   only because table cells have no spaces around `|`. The table above instead strips Markdown syntax from both sides
   before counting, which is why the PDF figures differ from Background (5175, 2366, 6021). The Optimal PDF figure
   excludes 112 words of running headers, footers and page numbers. A word-by-word diff of the remaining differences
   found hyphens restored at line breaks, punctuation spacing, box-drawing characters grouped differently, and the
   two spacing differences listed under Known limits. No content words are missing.
3. Passed. "Confidential Research Synthesis" occurs 0 times, and no page-number lines remain.
4. Passed. U+200B occurs 0 times.
5. Passed. No table changes column count between rows. The `Object / Purpose / Mutable?` and
   `Pattern / Consistency / Ops complexity / Verdict` tables are intact. That second header has one more column than
   this spec named.
6. Passed. `471.57`, `41.47` and `92%` appear in the right sentences.
7. Passed. Optimal page 5, Architecture page 3 and Reference Architecture pages 22-23 match their rendered pages in
   text and order. Headings on those pages match too.

Also checked: an independent script compared every PDF link's label and URL with the Markdown and found none missing,
extra or nested. A second run gave byte-identical files. `mypy --strict` passes, and an empty input directory gives a
FAIL line and exit 1. These checks were one-off scripts, not kept in the repo.

Decisions the raw converter output forced:

- **`header=False` isn't used.** pymupdf4llm's option also deleted body text at page edges in the Reference
  Architecture, including a closing SQL fence, a `Retrieval pseudocode:` line and whole paragraphs. The script instead
  removes lines that repeat on at least half of the pages.
- **Links are restored from the PDF.** The converter dropped all 80 URLs. Labels such as `arxiv` repeat with different
  targets, so each link goes on the occurrence on its page whose neighbouring words best match the PDF. An earlier
  version placed links in order and gave citations the wrong URLs.
- **Other repairs:** words joined at line breaks (34 repairs, for example `singlesession` to `single-session`), 31
  paragraphs and false bullets rejoined, 14 code blocks rejoined across page breaks, pipes escaped inside table cells,
  `<br>` removed from cells, and the 60 Works cited entries split onto their own lines.
- **The architecture diagram is rebuilt** from character coordinates into a fenced block, because the converter
  scrambled it.

Known limits:

- The diagram keeps the source's own defects: wrapped lines (`SDKs`, `(proposals)`) and the truncated word `ectors`.
  Its box lines don't align exactly with the text, as in the PDF.
- Two small spacing differences from the PDF remain: `PRIMARY KEY ( assertion_id, ...` in a SQL block, and a space
  added in `with →`.
- Source text is unchanged, including apparent typos such as `zep:atemporal` in the Works cited list.
- The repairs are tuned to these three PDFs, for example `Works cited` is a hard-coded heading. This is a one-off
  import, not a general pipeline.

Follow-up: the user decides whether `research/` can be published, given the "Confidential" footer and the public repo.
