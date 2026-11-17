---
id: 086
name: action-attribution
description: "Show who did what, by name, after a reload: job actions, page edits and proposals"
---

# 086 action-attribution

**Depends on:** 041, 044.

## Goal
The console can say who did something, by name, and still say it after a reload.

## Background
Two console screens found the same gap. Spec 041's "Cancelled by" and "Retried by" lines go on reload, because the event log records the actor (`crates/memex-types/src/event.rs`) but no route shows it. Spec 041 proved attribution only by querying the database. Spec 044's attribution shows a principal's id, not a name, because listing principals needs admin, and each history row reads its changeset for attribution, at most 50 per screen.

## Approach
- A read route, at read scope, that gives a job's or an object's actions with their actor. Only what this needs is added, and what it exposes follows the same safety rules as the activity stream's frames.
- A way for a read-scope user to see a principal's display name and whether it's a person or an agent, without listing all principals or exposing tokens or scopes.
- Spec 041's job screen and spec 044's page history use them. An actor field on `RevisionDto` replaces per-row changeset reads, if that's the simpler way.
- A deleted or unknown principal shows as such.

## Verification
1. A job cancelled and retried from the console shows both actors by name after a reload, in Playwright against a real server, with no SQL.
2. Page history shows names for a person and an agent, using a read-scope token, with the number of requests per screen recorded.
3. A read-scope token gets a name and kind but no token material or scopes, and a token with no access to the vault gets a refusal.
4. A principal that no longer exists shows clearly rather than as an error.
5. A name holding HTML renders as plain text.
6. `e2e:ingest` and `e2e:wiki` pass, and the API types are regenerated.

## Out of scope
A full audit log screen and editing names.

## Result
Built: a job keeps who cancelled or retried it, and a read-scope route names any actor in a vault. The job screen and page history show names from these, and they survive a reload. **The browser checks (items 1, 2 and the e2e half of 6) and the Postgres test haven't run yet.** They need the `memexvault` binary, and `pdfium-render` doesn't compile on this machine yet ("library kind `framework` is only supported on Apple targets"; no static Linux pdfium is pinned). That question is open with the coordinator.

**Built**
- `memex-types`: `JobRow.actions`, a list of `JobAction { action: cancel | retry, actor, time_ms }`, oldest first, capped at `MAX_JOB_ACTIONS` = 20. It's `serde(default)` and left out when empty, so stored rows and their JSON are unchanged until someone acts: no migration, and no scan of the event log. `ActorCard { id, name, kind, disabled }`.
- `memex-core`: `retry_job` and `cancel_job` record the caller on the row they change, and the log's event for that change names the same actor. `Server::actor` needs `VaultContentRead` on a vault that exists, then names the id. An id that names no principal gives `kind: unknown` and no name, not an error, since a job's actor can be a worker's id or a principal that's gone.
- `memex-api`, `memex-http`: `JobDto.actions` and `GET /api/v1/vaults/{id}/principals/{pid}`, giving `ActorDto { id, name, kind: human | agent | unknown, disabled }`. It holds no token, hash, scope, grant, admin flag or creator. There's no list: you name an id you've already seen in the vault.
- The activity stream's frames are unchanged. `JobFact` still leaves out the actor, and the frame safety test now puts its marker in the actions too.
- Console:
  - `core/actors.ts` turns an actor into text, with "Unknown principal …" for one that's gone, and "(disabled)".
  - `ActorName` reads each distinct actor once, cached for 5 minutes, and renders the name as a text node with a Person, Agent or Unknown badge.
  - The job card lists "Cancelled by" and "Retried by" with names and times, from the job itself.
  - Page history and the reader's byline use `RevisionDto.creator` and `ActorName`. The per-row changeset reads (`getWikiAttribution`) are gone, because the revision already names its proposer, so `RevisionDto` needed no new field.
- `e2e:ingest` item 4 now reads actors from the job route and the screen after a reload, with no `psql`. `e2e:wiki` has a new test for items 2, 3, 4 and 5.
- API types regenerated (`pnpm api:gen`) from `memex_http::openapi()`, the same document the server serves at `/api/v1/openapi.json`, through a throwaway example that isn't committed. The diff holds only the two schemas, `JobDto.actions` and the one path.

**Requests per history screen.** Before: 1 for history, plus 1 changeset per row, at most 50. After: 1 for history, plus 1 per distinct actor, cached across screens. That's 0 changeset reads, and 2 principal reads for a page of 6 revisions by a person and an agent. This comes from the code. The browser count in `e2e:wiki` asserts it but hasn't run yet.

**Verification**
1. Not run, as it needs the binary. `e2e:ingest` item 4 reloads after cancel and retry and expects both names as Person, with the API giving `[cancel, retry]` by the operator. In memory, `memex-core/tests/jobs.rs` passed: actions cancel, retry, cancel by the writer, in time order, and the log's events name the same actor. `memex-http/tests/api.rs` passed: the job's actions over HTTP, and the actor named.
2. Not run in the browser, as it needs the binary. The fake API and unit tests cover the names.
3. Passed over HTTP in `a_reader_sees_an_actors_name_and_kind_and_nothing_else`. A read token gets the name and kind. For the admin, no admin flag is given, and the body has no `token`, `scope`, `grant`, `admin`, `hash` or `created_by`. A principal with no grant on the vault gets 404, as do another vault and a vault that doesn't exist. No token gets 401. The scope matrix gives OK for read, write and admin, and 404 with no access. The browser half is in `e2e:wiki`, not run.
4. Passed over HTTP: an unknown id answers 200 with `name: null, kind: "unknown"`. In `core`, `actorText` shows it as "Unknown principal <id>", "no longer exists, or never was a principal", with a warning badge. The API has no way to delete a principal, so "gone" is an id that names none. Disabled is a real case, and shows "(disabled)" (browser check not run).
5. Passed over HTTP: a name holding `<img onerror>` and `<b>` comes back as the exact string. React renders it as a text node. The browser check, which expects no `img` or `b`, no `window.__pwned` and the literal text, is in `e2e:wiki` and not run.
6. API types regenerated. `pnpm typecheck` passed (types match the document), as did `lint`, `test` (725 in 42 files), `build`, and `budget` (140,029 bytes gzip against 200,000). `e2e:ingest` and `e2e:wiki` not run.

**Rust checks**
- `cargo fmt --check` passed.
- `cargo clippy --all-targets -D warnings` and `cargo test` passed on the workspace without `memex-pdf` and `memexvault`: 877 passed, 0 failed, 5 ignored. Those two crates don't compile here because of pdfium.
- The Postgres check in `pg_job_lease_renewal_cancel_and_takeover`, that the cancel's actor reads back from Postgres, isn't run.

**Limits and follow-ups**
- Cancels and retries made before this change have no recorded actor, though their log events still name one.
- Only cancel and retry are kept. Other object actions are already named by `RevisionDto.creator` and changesets.
