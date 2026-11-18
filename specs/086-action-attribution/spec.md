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
A job now keeps who cancelled or retried it, and a read-scope route names any actor in a vault. The job screen and page history show names from these, and they survive a reload. Keep. Measured on a debug server at 127.0.0.1:18186 on its own database, on a machine shared with five other workers, at load averages of 10 to 21.

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

**Requests per history screen** (`e2e:wiki`, read token, 6 revisions by a person and an agent): 8 API requests in all.
- 1 for the revisions.
- 2 for names, one per distinct actor.
- 0 for changesets.
- The other 5 are sign-in and shell requests: `whoami`, `health` twice, the vault, and the vault list.

Before this, each row read its changeset: 6 here, and up to 50 for a full screen. Names are cached for 5 minutes, so other screens showing the same actors don't fetch them again.

**Verification**
1. Passed, in `e2e:ingest` item 4 against a real server, with no SQL. The test cancels a running scan from the console and retries it. After a reload, the job shows "Cancelled by" and "Retried by", each with the operator's name and a Person badge. The job route gives `[cancel, retry]` by the operator for the scan, and `[retry, retry]` for a dead job retried twice. That job's page shows 2 "Retried by" rows. The Postgres test `pg_job_lease_renewal_cancel_and_takeover` passed: the cancel's actor reads back from the stored row.
2. Passed, in `e2e:wiki` (the "spec 086" test), with a read token. History names the person and the agent, with Person and Agent badges, and no name is still loading. It made 8 requests, as above.
3. Passed, over HTTP in `a_reader_sees_an_actors_name_and_kind_and_nothing_else` and on the real server in `e2e:wiki`.
   - The read token's lookup returns exactly `{id, name, kind: "human", disabled: false}`.
   - For the admin, no admin flag is given, and the body has no `token`, `scope`, `grant`, `admin`, `hash` or `created_by`.
   - A token with no grant on the vault gets 404, and the body doesn't hold the name. So do another vault and a vault that doesn't exist.
   - No token gets 401.
4. Passed. An id that names no principal answers 200 with `name: null, kind: "unknown"`, on the real server and over HTTP. The console shows it as "Unknown principal <id>", with "no longer exists, or never was a principal" and a warning badge (unit tests in `core/actors.test.ts`). The API can't delete a principal, so a gone principal is an id that names none. A disabled principal was disabled during `e2e:wiki`, and after a reload its rows say "(disabled)".
5. Passed, in `e2e:wiki`. The name `<img src=x onerror="window.__pwned=1"><b>Pat …</b>` shows as that literal text. History holds no `img` or `b` element, and `window.__pwned` is undefined.
6. Passed.
   - `e2e:ingest`: 5 passed and 2 skipped, in 1.9 minutes. The skipped ones are opt-in: item 1's recording (`MEMEX_E2E_RECORDING`) and the 5-minute load test (`MEMEX_E2E_LOAD`).
   - `e2e:wiki`: 18 passed, in 2.7 minutes.
   - The API types are regenerated with `pnpm api:gen` from `memex_http::openapi()`, the document the server serves. The diff holds only `ActorDto`, `JobActionDto`, `JobDto.actions` and the one path.

**First runs, and what changed.** No limit was changed.
- `e2e:wiki`: the new test failed once because of the test, not the screen. A row's text runs its link into the title ("Revision 1Named …"), so `/Revision 1\b/` matched nothing. It now finds rows by their exact link.
- `e2e:ingest` item 4: an older assertion expected "Cancelled by operator-…" as one string, but the Person badge now sits between the two. It now checks the action's `[data-actor-name]`.

**Other checks**
- Passed:
  - `cargo build --workspace`.
  - `cargo fmt --check`.
  - `cargo clippy --workspace --all-targets -- -D warnings`.
  - `cargo test --workspace`: 996 passed, 0 failed, 108 ignored, in 80 suites.
  - `pg_job_lease_` on Postgres: 2 passed, in 124 s.
  - `pnpm typecheck`, `lint` and `build`.
  - `pnpm test`: 725 tests in 42 files.
  - `pnpm budget`: 140,029 bytes gzip, against the limit of 200,000.
- Not run: `cargo deny check licenses`, as `cargo-deny` isn't installed here. This spec adds no dependency.

**Limits and follow-ups**
- Cancels and retries made before this change have no recorded actor, though their log events still name one.
- Only cancel and retry are kept on a job. Other object actions are already named by `RevisionDto.creator` and changesets.
