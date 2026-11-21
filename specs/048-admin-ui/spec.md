---
id: 048
name: admin-ui
description: Build vault, token, status, operations, erasure and MCP administration screens
---

# 048 admin-ui

## Goal
An operator can run the system from the console: vaults, tokens, status, maintenance and erasure, without needing the command line once the server is up.

## Background
**Depends on:** 039, 040. Design: `docs/architecture.md`, sections API conventions, Backends.

## Approach
- Vault create, rename and delete. Principals and tokens with scopes and expiry, where the token's value is shown once with a copy button.
- Status page for system health, Postgres and LLM reachability, lag, queue depth, sizes and caches, updating live.
- Operations: reindex, rollback, reconcile with its report, export, import and sweep, showing job progress.
- Erasure: an impact preview, a typed confirmation of the source's name, and approval through review, followed by a report of what was removed.
- Per-vault chat setting showing the model and endpoint that vault content would go to. Effective settings and backends, read-only. An MCP page with endpoint, tool list and a ready-to-copy client config.

## Verification
1. Vault, principal and token journeys pass in Playwright, and a revoked token stops working at once.
2. A new token's value is never shown again after the dialog closes, and appears in no later API response or log.
3. Secrets are shown as redacted on the settings page, using the markers from spec 039.
4. An erasure from the UI shows a preview that matches the result, and needs human approval.
5. Destructive actions can't run with a single click, shown by a test of every one.
6. Every admin screen shows a clear permission message for a non-admin principal, and the API refuses it as well.
7. With Postgres down, the status page explains the problem instead of failing.

## Out of scope
Runtime editing of settings, user self-registration and billing.

## Result
Done. Measured on 2026-11-21 on Linux x86_64, against a pgvector 0.8.7 / Postgres 18 container of this worktree's own (podman,
`127.0.0.1:55448`), as nothing was serving 55432, with a fresh database per run and a debug build of the binary embedding the console. No LLM
or provider was called: the LLM and the transcription API were configured, so their keys show on the settings page, but pointed at a
closed port on this machine.

**What was built.** An Admin section in each vault, a tab per screen, all in its own lazy chunk (13.4 kB gzip):
- *Vault*: rename, the chat setting with the model and endpoint that vault content would go to, and delete. *All vaults* (server admin):
  every vault and a create form. A server admin with no vault creates the first one on the home page.
- *Tokens*: create a token for yourself or, as a server admin, for any principal, with grants on the vaults you administer and an expiry in
  days; its value is shown once with a copy button. Your tokens, or all of them for a server admin, with their state, and revoke. Principals
  (server admin): add, and disable.
- *Operations*: reindex, roll back, reconcile (deep, repair, up to an event), export, sweep and compact, each followed to its end with its
  progress and report, and the export's archive to download. Import of an archive (server admin), under a new id or the archive's, with replace.
- *Erasure*: pick a document, see the preview by table, type its name to propose the erasure, follow it through Review, and see the result
  compared with the preview it was proposed from. A history of erasures.
- *Status* (server admin): health first, worst first, with what each problem means; version, memory, models, each vault's head, index lag,
  queues, review and files; caches with clear; backends and thresholds. It reads `GET /system/status` every 2 s and nothing else, so it
  keeps working with Postgres down. *Settings* (server admin): the effective settings with spec 039's markers shown as secrets, and the LLM
  usage report by day, vault, agent and model, with cost only where priced. *MCP* (server admin): endpoint, transports, protocol versions,
  hosts, tools, and ready-to-copy HTTP and stdio client configs with a placeholder for the token.
- `core`: `admin.ts` (tab access matching the server's rules, `confirmationMatches`, the operations table, `settingRows`, `healthLines`,
  `tokenState`, `expiryFrom`, `mcpClientConfig`, `compareErasure`) and `admin-cases.ts` (`AdminApi` and the engine's cases, reading every
  page of principals, tokens, operations and erasures up to 20). Translation in `api/translate-admin.ts`, `FetchApi` methods in `edge-http`
  (an import is a raw `fetch` of the file), and a fake in `testing-admin.ts`. No API change, so `openapi.json` is unchanged.
- Decisions, recorded in `docs/frontend-architecture.md`, Administration: every change takes two presses, the first sending nothing, and a
  destructive one also needs its object's name typed; a new token lives only in the page's own state, never the query cache; the status
  page polls rather than following the activity stream, as it's server-wide. None needed a decision the docs didn't already cover.

**Verification.** `pnpm --filter @memex/console e2e:admin`, 8 tests, all pass, in 1.2 min on the final run:
1. *Vault, principal and token journeys.* From the console: the first vault created on the home page, renamed, a second created from All
   vaults and deleted (404 afterwards); a principal added, a token made for it with a read grant and a 30-day expiry, used from the API and to
   sign in a second browser, then revoked: the next `whoami` with it answered 401, 2 ms after the revoke's answer, and the second browser's
   next load went to sign-in. A disabled principal's token was refused at once too. Passes.
2. *A new token's value is shown once.* After the panel closed, the page, the page after a reload, `GET /tokens?all=true`, all 7 API answers
   the console read after the creation and 164,563 bytes of server log held it 0 times. Passes.
3. *Secrets redacted.* The database role's password was a marker, and the LLM's and the transcriber's keys and the LLM URL's user info and
   query were the same marker. The settings page showed `llm.api_key` and `transcribe.api.api_key` as `secret, set`, `mcp_token` as
   `secret, not set`, and `database_url` and `llm.base_url` with `<redacted>` as `credentials hidden`; the settings and vault pages, 6 admin
   answers (settings, status, MCP, usage, chat settings, tokens) and 179,475 bytes of server log held the marker 0 times. The chat setting
   named the endpoint as `http://127.0.0.1:9`, with no credentials. Passes.
4. *Erasure.* A text document's preview showed 7 rows removed, 0 rewritten, 0 flagged, the same as the API's preview. Proposing it with the
   name typed left the document readable and the ChangeSet pending; a second admin approved it in Review, and 2.3 s later the first page
   showed the report, "The result matches the preview exactly": the record's removed, rewritten, flagged and source key sets equal the
   preview's, its actor is the approver, and the document answers 404. Passes.
5. *No single click.* For each of delete a vault, revoke a token, disable a principal, propose an erasure, roll back, sweep, compact,
   reconcile with repair, import replacing a vault, clear caches, turn chat on and turn chat off: the first press opened the confirmation and
   sent no request in the following 300 ms; for those with a typed name, the button stayed disabled with a wrong name, Enter included, and no
   operation existed afterwards. A reconcile and a compact then ran to `done` with their step and report shown and matching the jobs API, and
   an export's archive downloaded. Passes.
6. *Permissions.* Read and propose tokens: no Admin link, and all 8 admin URLs show "You don't have permission to use Admin in this vault."
   A vault admin: the 4 vault tabs, and on the 4 server tabs "You don't have permission to use Status here." and so on. The API refused all
   three 23 reads and 17 writes (403). Passes.
7. *Postgres down.* With the status page open, the proxy in front of Postgres cut every connection: 1,786 ms later the page showed
   "Postgres is down" first, with what it means, "Signed in from memory" and the vault figures as unavailable, and kept updating; when
   Postgres returned it read reachable again 781 ms later. Passes.

Also: axe finds no violation on the 8 admin screens, and the browser logged no CSP or script error in any test.

Other checks: in `web/`, `pnpm test` (795 tests in 49 files, 33 new), `pnpm typecheck`, `pnpm lint`, `pnpm build`, `pnpm run licenses`,
`pnpm run contrast` (53 pairs, 0 failed), `pnpm budget` (sign-in route 150,894 bytes gzip of 200,000), `pnpm api:check` against the
server and `pnpm e2e` (6) pass. `e2e:shell` passes 17 on a fresh database: its sign-out test now waits for the Admin screen's reads before
signing out, as Admin, the last screen it opens, now reads something, and a read still on its way when the session ends got a 401 the browser
logged. `cargo build --workspace`, `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo deny check licenses`
and `cargo test --workspace` pass (1,037 passed, 126 ignored, with `MEMEX_PDFIUM_LIB` set). A first run, beside another worker's tests, failed memex-http's
`worker_progress_reaches_a_stream_with_no_id` once on a stream timeout; it passed three times alone and in the full rerun. No Rust changed, so the
Postgres suites weren't rerun.

**Left as found.** The other screens' e2e suites weren't rerun: only the shell's code path around them changed, and the shell's own suite
covers that. Imports are held in the server's memory (spec 039), so the import panel loses track of one after a server restart.
