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
A user can read and edit pages with `[[links]]` that autocomplete, see backlinks and history, and can't silently overwrite someone else's change. Keep. Everything in the Approach is built, on the real backend, with no change to `crates/`. Timings and flakes below are from a machine at a load average of about 900, with other workers on the same Postgres.

**Built**
- `core`: `wikitext` (the server's slug, block and inline rules, and what the cursor is inside), `wiki-pages` (ranking, slug index, tree, filter), `merge` (three-way by block), `draft` (`DraftKeeper` on a new `DraftStorage` edge), `wiki-routes`, and `wiki-cases`, the engine's use cases. A new `IdSource` edge supplies idempotency keys.
- `api`, `edge-http`, `edge-browser`: translation, the page routes, `browserDraftStorage`, `browserIds`. `fakeApi` has an in-memory wiki.
- `packages/ui`: `SafeMarkdown` in `@memex/ui/markdown`, the one sanitizing component. It knows nothing about pages: a caller passes placeholders and how to draw them, so chat can use it as it stands. `EpistemicBadge` gains `verified`.
- `apps/console/src/wiki/`: list and tree, reader, editor with split preview, conflict view, history, diff, broken links, and rename, delete, revert, import and export.
- `pnpm --filter @memex/console e2e:wiki`: 17 browser tests on a real debug server.

**CodeMirror and the CSP.** Measured early, in Chromium against the real server and its policy, on a throwaway build.

| Mounted | Stylesheets | `<style>` elements | `.cm-editor` display | Scroller font | Tooltip background | CSP errors |
|---|---|---|---|---|---|---|
| In the document, by default | 0 adopted | 1, blocked | `block`, so broken | the page's | transparent | 1 |
| In a shadow root, through CodeMirror's `root` option | 1 adopted, 116 rules | 0 | `flex` | CodeMirror's | styled | 0 |

`style-mod` uses a constructable stylesheet only for a shadow root, and the CSP doesn't apply to those. So the editor lives in one, with the policy unchanged: `default-src 'self'; base-uri 'none'; form-action 'self'; frame-ancestors 'none'; object-src 'none'`. With our theme it's 132 rules. The design tokens are custom properties, which cross the boundary. **Only Chromium was measured.** Firefox and Safari weren't run, as the project's Playwright runs only Chromium.

**Modals: 0 new, so 0 new CSP errors.** The conflict view and the rename, revert and delete confirmations are in the page, and every flow test asserts no console error. `packages/ui`'s modals are unchanged, and `e2e:shell` still measures 3 errors for 3 palette openings. The user's decision from spec 40 is still open, and nothing here depends on it.

**Bundle.** `pnpm budget`: the sign-in route's initial JavaScript is 117,702 bytes gzip (364,980 raw), against the limit of 200,000. That's 8,073 more than spec 40's 109,629, from how the bundler split shared code: no wiki code is in it, and a test confirms that in the browser. The cause of the rise wasn't looked into further. The wiki's chunks, by gzip: `wiki-screen` 66.5 kB (218.7 raw), and `markdown-editor`, which is CodeMirror, 184.4 kB (531.8 raw). A test shows the list doesn't load CodeMirror, and the editor does.

**Verification**
1. Passed. Create, edit, a link chosen from autocomplete by keyboard, rename and revert, in one test on the real server. After a rename, a link through the old slug still reads as resolved and leads to the page, and `by-slug` with the old slug lands there too. The backlink shows on the target, and a revert gave revision 4 with exactly the first text.
2. Passed, with two browser contexts and two tokens. B's stale save shows both versions and writes nothing: the revision stayed 2 and the body stayed A's. Saving is disabled until the one real conflict has a choice. A third writer landing during the merge makes the merge itself stale, and shows the view again with nothing overwritten. The final body, checked exactly, keeps all three people's text. "Use theirs" drops the draft too.
3. Passed. Log: `specs/044-wiki-ui/hostile-network-log.json`. Over the reader, the saved page's preview, typed text and a restored draft, in two runs:

   | Run | Requests after sign-in | Outside the origin | `window.__pwned` | Dialogs | CSP violations | Page errors |
   |---|---|---|---|---|---|---|
   | Real CSP | 65 | 0 | null | 0 | 0 | 0 |
   | CSP bypassed, so only the sanitizer stands | 66 | 0 | null | 0 | 0 | 0 |

   The DOM holds no `script`, `img`, `iframe`, `object`, `embed`, `video`, `audio`, `link`, `style`, `meta`, `base`, `form`, `srcdoc` or `on*` attribute, and every `href` is `http(s)`, `mailto` or a same-site path. **The check can fail:** a script and an image added by hand to that same page gave 2 CSP violations with the policy on, and with it bypassed the script ran and the image was requested.

   The fixture is `e2e-wiki/fixtures/hostile.md`, and its title holds an `onerror` too:
   - **Script tags:** block, inline, with a remote `src`, inside `svg`, and written with HTML entities.
   - **Event handlers:** `img onerror` (remote and local), `svg onload`, `video onplay`, `input onfocus` with `autofocus`, `div onclick` and `onmouseover`, `b onclick`.
   - **`javascript:` links:** raw `<a>`, Markdown, mixed case, entity-encoded, percent-encoded newline, leading space, autolink, reference-style, as a wiki target. Also `vbscript:`, `data:text/html` and `file:`.
   - **Remote loads:** raw `img`, Markdown image, with a title, reference-style, inside a link, `data:` SVG, in a table, in a wiki label. Also `iframe`, `srcdoc`, `object`, `embed`, `video`, `audio`, `link rel=stylesheet`, `<style>` with `@import` and `url()`, a `style` attribute, `meta refresh`, `base` and `form action`.
   - **Inert on purpose:** the same text in inline code and a fence, and a `{{claim:` holding a script.
4. Passed, for `basics`, `links` (resolved, old slug, label, broken, escaped, malformed, inside a Markdown link, in a table), `claims` (current, replaced, missing, repeated, escaped), `awkward` (no heading, an unclosed fence, stray characters) and `hostile`. For each, the rendered HTML was exactly equal three ways: the unsaved draft's preview, where each claim came through the lookup a draft uses, the reader, and the saved page's preview. The evidence panel was compared too. Beyond the spec, the console's parser found the same references as the server's stored links on each of the 5.
5. Passed, three repeats on a separate run as well. It survives a reload inside the pause before a write, and closing the tab with no chance to flush. A save removes it and it doesn't come back. A new page's draft keeps its title, and discarding clears it. A draft from before someone else's save warns, and saving it shows the conflict view.
6. Passed. `pnpm test`: 191 tests in 14 files, 78 of them new, in plain Node on `ManualClock`. Link autocomplete: what the cursor is inside (escapes, code, fences, labels, two openers, blank lines), ranking and closers. Drafts: the pause and longest wait, flush, discard, age, failed writes and bad storage contents. Bad cases also cover a lost answer retried, a stale write, cursors that never end and a cycle of replaced claims.

Also passed in `e2e:wiki`: claims with status, trust tier and evidence; claim autocomplete; agent and human attribution, and an agent's held edit; a read token; export and import through real files; list, tree and filter in the URL; missing pages and failed saves that keep the text; axe on the list, tree, reader, editor, conflict, history and diff; keyboard use with no overflow at 1024 px.

**Decisions**
- **The tree comes from slugs.** Pages have no parents, so `alpha-notes` sits under the page `alpha` when one exists. It needs no new data, and a filter shows a plain list.
- **References are taken out before Markdown sees them,** using a TypeScript copy of the server's rules, so the same things are links on both sides. A Markdown plugin would read `[[a|b]]` in a table as two cells. If `memex_core::page` changes, `wikitext.ts` has to change too: `wikitext.test.ts` holds the server's cases, and item 4 compares the two.
- **Raw HTML shows as text** rather than vanishing, so a reader sees what's on the page. Images show a marked placeholder with their address, and are never requested.
- **The sanitizer is an allowlist of what Markdown makes,** not the library's default with parts removed.
- **A retried write reuses its key** until it gets an answer, so a lost answer replays rather than conflicting with itself. The server checks the key before the version.
- **A conflict with identical text counts as saved,** as there's nothing to choose.
- **A merge pairs off blocks edited in place** when all three sides have the same count, as the server's block matching does. Without that, neighbouring changes become one conflict. A delete against an edit is always a conflict.
- **A draft goes once the server has the text,** including when it's held for review, because the changeset then holds it. A refused or failed save keeps it. Drafts expire after 30 days and are per person, vault and page.
- **The editor reads the page afresh** before starting from it, and its base version never changes after that except by a merge.
- `remark-gfm` and `@lezer/highlight` are added, both MIT and recorded in the frontend document. `pnpm run licenses` passes.

**Findings**
- **Occasional stalls on this machine.** In 3 of about 8 full or partial runs, one fresh page load sat for over 45 s with the offline banner or `Loading` showing, and the test failed, in a different test each time: items 3, 4 and 5. It's not reproduced in isolation, and the same tests pass on rerun. Postgres connections were sampled at 65 to 67 in a later run, at the shared limit of 64, so a 503 from `whoami` and `/health` during saturation fits what the page showed. **That cause isn't proven,** as no run captured both the stall and its responses. The wait in `e2e:wiki` is 45 s. `e2e:shell`'s default of 5 s also failed twice in one run at this load, on screens this spec didn't touch, and those two tests then passed 6 of 6.
- **The page route doesn't report `restored_from` for the current revision,** as `view_of_page` always gives none. The reader takes it from the revision's row, so no backend change was needed. An ask for `crates/` if anyone wants it fixed there.

**On the final code,** in one pass with exit 0 for each: `e2e:wiki` 17 of 17, `e2e:shell` 17 of 17, `e2e:session` 7 of 7, `pnpm e2e` 6 of 6, `pnpm test` 191 of 191, and `typecheck`, `lint`, `licenses`, `contrast` (53 pairs, 0 failed) and `budget`.

**Not run.** `cargo test`, clippy and the `pg_*` tests, as no crate changed. `cargo build -p memexvault`, `cargo fmt --check` and `cargo deny check licenses` ran and pass. No LLM was called. Approving a delete through the console: the review screen is spec 046, so a delete was shown going to review, and not its approval.

**Follow-ups**
- **Attribution shows a principal's id, not a name.** `creator` and `proposer` are ids, and listing principals needs admin. Person or agent is shown clearly, but a name needs a small read route in `crates/`.
- "Open it in Review" goes to the placeholder until spec 046.
- Autocomplete and link states work from a list of at most 10,000 pages, as the API has no prefix search. Past that, the list says so, and a link not in it is neutral and resolves when followed.
- Each history row reads its changeset for attribution, at most 50 per screen. A field on `RevisionDto` would remove those calls.
- Run the editor once in Firefox and Safari.
