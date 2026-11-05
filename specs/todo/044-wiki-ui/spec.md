---
id: 044
name: wiki-ui
description: Build wiki page browsing and the Markdown editor with links, history and diffs
---

# 044 wiki-ui

## Goal
A user can read and edit pages with `[[links]]` that autocomplete, see backlinks and history, and never silently overwrite someone else's change.

## Background
**Depends on:** 022, 040. Design: `docs/frontend-architecture.md`, sections Decisions, Security.

## Approach
- Page list and tree, and a reader with rendered links, claim references with their evidence, backlinks, outgoing links and unresolved links.
- A CodeMirror Markdown editor with split live preview, autocomplete for `[[links]]` and `{{claim:<id>}}`, and a draft held locally through the `storage` edge.
- Saving goes through the page update API, which applies governance. A stale version shows a conflict view with both versions and a merge choice.
- Revision history, diff, revert, rename, delete, Markdown import and export. Agent-written and human edits are clearly attributed.
- One sanitizing Markdown component shared with chat.

## Verification
1. Create, edit, link, rename and revert pass in Playwright, and a rename keeps links working.
2. Editing from a stale version shows the conflict view and never overwrites, tested with two sessions.
3. A hostile page with script tags, `javascript:` links, event handlers and remote images runs nothing and loads nothing from outside, shown by a network log and a script-execution check.
4. Reader and preview produce the same output for a fixture set of pages, including links and claim references.
5. An unsaved draft survives a reload, and is discarded after it's saved.
6. Pure logic for link autocomplete and draft handling has unit tests, with `ManualClock`.

## Out of scope
A visual editor, live co-editing, page templates and page permissions.

## Result
Not started.
