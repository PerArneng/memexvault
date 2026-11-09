---
id: 082
name: job-lease-renewal
description: "Keep a running job's lease alive so long renditions finish, and report a missing blob as missing"
---

# 082 job-lease-renewal

**Depends on:** 014, 015, 016, 028.

## Goal
A rendition job that takes longer than one lease still finishes, so long documents can be ingested. A server that dies still frees its jobs within one lease. A missing blob is reported as missing, not as a backend that might come back.

## Background
Spec 042 found this while building its 400-page fixture. A job's lease is `LEASE_MS`, 60 s, in `crates/memex-core/src/jobs.rs`, and the generic worker loop in `crates/memexvault/src/workers.rs` never renews it. Extraction, review and audit extend theirs through `Server::hold_job`, and transcription renews its own every 60 s (`HOLD_MS` and `HOLD_EVERY` in `crates/memexvault/src/transcribe_job.rs`). Rendition jobs don't.

Measured by spec 042 on a release build at load about 7: a 400-page text PDF needs about 72 s, as hidden-text sampling OCRs every tenth page. Without a workaround it ran 5 attempts of about 60 s, each ending "the worker stopped without finishing", and went `dead`. Spec 042's item 5 kept the lease alive with SQL so its document could exist. What matters is the time a job takes, not its pages, so a scanned PDF that needs OCR on every page, or any document on a busy machine, will hit this sooner. Neither was measured.

Spec 042 also found that a source whose blob file is missing gets 503 "a backend is unavailable, so try again", through `ServerError::Unavailable` in `crates/memex-http/src/error.rs`. Retrying won't help.

This spec's wave runs after spec 038's, which adds progress reporting to the same workers.

## Approach
- One way for any long job to keep its lease, shared by the generic worker loop and the transcription worker, renewed well before the lease runs out. A renewal that fails, because the job was taken or cancelled, stops the handler's work and writes nothing, as completion already does.
- Rendition jobs, and any other job kind run by the generic loop, use it. Check each job kind and record which ones can pass one lease.
- Keep a dead server's jobs reclaimable within one lease, so a renewal is only made by a handler that's still running.
- A missing blob becomes its own error, mapped to a clear status and message that say the file is missing, and the console's document screen shows that message instead of its own guess.

## Verification
1. The 400-page text PDF from spec 042 ingests on a release build with no keep-alive, and the job's attempts and time are recorded with the load.
2. A rendition job made to run for at least 3 leases on ManualClock or a scripted handler finishes in 1 attempt, and a second worker can't claim it meanwhile.
3. A server killed during a long rendition frees its job within one lease, and the next attempt finishes.
4. A handler whose job was cancelled or taken during a renewal stops and writes nothing.
5. A source whose blob file is deleted gets the new error through HTTP, with a status that isn't 503, and spec 042's bad-case check shows the server's message.
6. Spec 042's `e2e:documents` passes without its SQL keep-alive.

**Pass rules, set before measuring.**
- **Item 1:** on a release build through `memexvault serve`, with no keep-alive, the 400-page PDF's rendition job ends `done` with exactly 1 attempt, and the source has 400 pages. The attempt has to last over 60 s to show anything. If it doesn't, the run still counts as ingested, and it's recorded as not having tested the lease. Time and load are recorded, with no limit on the time.
- **Item 3:** kill -9 a `memexvault serve` process during the same PDF's rendition, once its lease has been renewed at least once. At the kill, `lease_until_ms` is at most 60 s ahead. A new `serve` started straight away gives the job a second attempt no sooner than that time and no later than 60 s after the kill plus the new server's start-up and 5 s for polling. That attempt ends `done`, and the first attempt reads "the worker stopped without finishing".

## Out of scope
Making OCR or hidden-text sampling faster, changing `LEASE_MS` itself, and recovering a deleted blob.

## Result
Built one way for a job to keep its lease, used by every job kind, and a missing blob as its own error. Items 1 to 6 pass. Migrations: none. New dependencies: none. `LEASE_MS` stays 60 s.

**What was built.**
- `crates/memexvault/src/workers.rs`: `run_one` starts a keeper thread beside each handler. It renews every `RENEW_EVERY_MS` (20 s, a third of the lease, in `jobs.rs`), counted from the claim, and goes by the server's clock, so ManualClock drives it. A job under 20 s makes no renewal and writes nothing extra.
- The keeper ends when the handler returns and is joined before the result is recorded, so no renewal can land after it. It lives in the process running the handler, so a dead server stops renewing.
- `JobHandler::run` now takes a `JobHold`, which reads as lost once a renewal says the job was cancelled or taken. A store error isn't that: the keeper tries again after 2 s, and the fence on completion decides.
- `Server::hold_job` no longer takes a length. Every hold is for one `LEASE_MS`. `HOLD_MS`, `SCAN_HOLD_MS` and `LEASE_MARGIN_MS` are gone, and transcription uses the keeper instead of its own loop.
- `ServerError::BlobMissing` comes from `BlobStoreError::NotFound`, and gives status **410** with code `blob_missing` and the message "the stored file is missing from the server, so trying again won't help until it's restored". The console's document screen shows the server's message in place of its own guess.
- The OpenAPI document has 410 on the 10 routes that read a file, and names the code in `ErrorDetail.code`. `web/packages/api` was regenerated from a running server.
- `/stream` checks for the file before it answers, as it couldn't change its status once streaming.
- A rendition or transcription job whose file is missing now goes `dead` at once, in 1 attempt, instead of retrying. Extraction treats it as fatal too.
- `specs/082-job-lease-renewal/tools/long_job.py` runs items 1 and 3 on a real server.

**Measured.** All on release, with the machine shared with four other workers. Load averages are the 1, 5 and 15 minute figures, so these timings are weak: they say what happened on a busy machine, and nothing about a quiet one. The pass rules are under Verification, and were committed with the code, before the first measurement.

1. **Passed: `done`, 1 attempt, 91.6 s, 400 pages,** at load 164.6 / 158.0 / 122.0 at the start and 103.7 / 141.3 / 119.4 at the end. Total from upload was 93.1 s. Over 60 s, so it did test the lease. Spec 042 saw 72 s at load about 7 and 5 failed attempts. A second run inside `e2e:documents` took 86.5 s in 1 attempt at load about 93 to 187.
2. **Passed,** in `workers.rs` on MemStore and in `pg_job_lease_renewal_cancel_and_takeover` on Postgres. A scripted handler runs for 200 s on ManualClock, which is 3.3 leases, in 10 steps of one renewal each. A second worker's claim returned nothing at all 10 steps, and the job ended `done` in 1 attempt. Another test checks that each renewal is for exactly one lease.
3. **Passed, on real servers.** `kill -9` of `memexvault serve` during the 400-page rendition, after 1 renewal. The lease had **58.7 s** left at the kill, so at most one lease. A new server was ready in 2.6 s. The second attempt started **58.74 s** after the kill, 17 ms after the lease ended, and ended `done` in 89.4 s. The first attempt reads "the worker stopped without finishing". Load 97.3 / 138.7 / 118.8 at the start and 150.1 / 141.3 / 123.3 at the end. The kill's orphaned OCR child wasn't traced: it ends by itself when its page is done. `pg_job_lease_a_killed_process_frees_its_job_within_one_lease` does the same with no models, on real 60 s leases. It renewed 3 times in 65 s, and the job was claimed 55.2 s after the kill, 20 ms after the lease ended, and again at 55.5 s, 107 ms after, in a second run.
4. **Passed,** on MemStore and on Postgres. A job cancelled while running: the handler hears at its next renewal, its result is refused, and the job stays `cancelled`. A job taken after a stall: the same, and the new holder's lease and state are untouched, and it completes. Real handlers: a lost rendition kills its parser in under 5 s and reads no more OCR pages, a lost transcription kills its child, and neither builds a rendition, so no blobs are written after that. An embed job that has gone stores nothing. The same store failure mid-job doesn't stop the handler. **Limit:** a job is told at its next renewal, so at most about 20 s after a cancel. Extraction, review and audit can't interrupt a model call, so they stop at their next `hold_job`, as they did before, and write nothing further.
5. **Passed.** A deleted blob on the real server gives **410** with `blob_missing`, and the screen reads "Couldn't show page 1. the stored file is missing from the server, so trying again won't help until it's restored (request id: ...)" with a Retry button. Retry stays, since the file can be restored. The row still answers 200, which is what tells this from an erased source's 404. `a_missing_blob_says_so_and_is_not_a_backend_to_retry` and `the_openapi_document_has_the_missing_blob_on_each_route_that_reads_a_file` hold this in `cargo test`.
6. **Passed: `e2e:documents` 11 of 11, in 12.8 min, with spec 042's SQL keep-alive removed** and a check added that the 400-page job took 1 attempt. It ran against a throwaway Postgres container on port 55482, since removed, with a data folder of its own. Chromium only. At load about 90 to 220: item 5 of spec 042 opened in 769 to 960 ms, median 890 ms, under its 1,000 ms rule; playing from a citation had 0 ms position error, with medians of 96 ms from a click and 761 ms from a fresh load; and the 5-minute recording transcribed through `serve` in 19.6 s, on models linked read-only from the main checkout.

**Which job kinds can pass one lease.** All of them can, and all now renew through the worker loop.

| Kind | Can pass 60 s? | Why, and what bounds it |
|---|---|---|
| `rendition` | Yes, measured at 91.6 s | Parser limit 300 s, and 120 s for each OCR page. A scan that needs OCR on every page wasn't measured, but only time matters now. |
| `transcription` | Yes, and it always renewed | Hours of audio, with limits on stall and memory. |
| `extract` | Yes | One or more model calls, each up to 120 s, plus a retry. |
| `review` | Yes | One call, up to 60 s, plus a retry. |
| `audit` | Yes | A scan of the whole vault, then calls of up to 120 s. |
| `embed` | Yes, under load | At most 32 texts in a job, so normally seconds. Not measured past 60 s. |

**Decisions.**
- **Extraction, review and audit held their jobs for longer than a lease, up to 15 minutes for an audit's scan, and that's gone.** The Goal says a server that dies frees its jobs within one lease, and the keeper makes those long holds unnecessary. So a dead server's audit now comes free in 60 s rather than 15 minutes, and a transcription's in 60 s rather than 5 minutes. No test had to change for this except one that waited out a 1 s hold.
- **410 Gone,** rather than 404, which the screen reads as an erased source, or a 5xx, which reads as worth retrying. Spec 030 should keep to this: a row that's gone is 404, and a row whose file is missing is 410.
- The keeper is one thread for each running job, not one for the server, because it's simple and a job's life bounds it. Threads wait on a channel, so a short job costs a thread start and nothing else.
- A job's renewal is a `state.recorded` event, as every hold has been, so a job writes one event every 20 s. An hour's transcription writes 180, against 60 before.

**Limits, not fixed.**
- **A handler that hangs inside this process now holds its job until the server restarts,** as the keeper can't tell a stuck handler from a slow one. Before, its lease ran out and another worker ran the job. Children have time limits and model calls have timeouts, so this needs a hang in code running in-process, such as the embedder. A ceiling on a job's total time would cover it, and isn't built, as nothing in the spec sets one.
- A pause over 40 s in one process, such as a machine that sleeps, loses the lease, and that's as intended: the job runs again, and the old result is refused.
- The lease can't be renewed during an outage of the store, so a job can be taken if the store is away for over 40 s and comes back before the keeper's next try. **Found at landing: a Postgres restart is worse than this.** The server's writer lease for each vault is cached and never regained (`Server::lease`), so after a restart every write to an existing vault fails until the server restarts, and the job stays `running`. That was already so on `main` before this spec, and is spec 089.

**Definition of done**, on the final code: `cargo build --workspace` and `cargo test --workspace` (990 passed, 0 failed), `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check` and `cargo deny check licenses` pass. From `web/`: `pnpm typecheck`, `lint`, `test` (373), `run licenses`, `run contrast` (53 pairs), `e2e` (6) and `budget` (120,069 bytes gzip against 200,000) pass. `e2e:wiki` and `e2e:shell` weren't run, as nothing they use changed.

**Postgres tests:** the brief's command, on a freshly dropped and recreated database each run. **The third run passed all 49, with no failures,** in 17 min at load about 88 falling to 17, the only run where other workers weren't filling the server's 64 connections. The two earlier runs, at load about 70 to 190, failed on connections shared with other workers: 27 tests in the first and 7 in the second.
- Each run's failures were almost all `53300: too many clients`, and each test that failed passed in the third run.
- Three of them failed some other way, on a run that also hit that error. `pg_kill_jobs_200_times` ended with 1,294 jobs open. It passed with 0 open in the next two runs, at 1,498 and 1,494 done. `pg_contract` got something other than `WriterActive` from a second `open_vault`, and `pg_page_new_pages_are_found_within_the_lag_target` had a p95 of 1,134 ms against its 1 s target. **The cause of the `pg_contract` failure wasn't confirmed**, and it passed in the first and third runs.
- Numbers from the third run: job kill harness, 200 cycles, 1,500 jobs, 1,494 done, 6 dead, 0 open, 0 duplicates. App kill harness, 1,000 cycles, lost 0, gaps 0.

**Not run,** as the brief says: `pg_bad_cases`, `pg_kill_postgres`, `pg_full_disk` and the `tls_` tests. `pg_child` and `pg_job_child` ran only as helpers of the kill harnesses. **`pg_kill_postgres` covers what this change touches,** as a Postgres restart now makes a running job's keeper fail and try again, and that's only tested here with a scripted store failure, so read it closely at landing. The others cover nothing this changes.

**Landing checks, by the coordinator.** On 0b7cd50, at load about 6 to 10: `cargo build --workspace`, `cargo test --workspace`, clippy, `cargo fmt --check` and `cargo deny check licenses` pass. The brief's Postgres command passes on a fresh database, with `pg_kill_jobs_200_times`, `pg_page_new_pages_are_found_within_the_lag_target` and `pg_contract` all passing this time. `pg_bad_cases` passes on a throwaway server. `pg_kill_postgres_100_times` passes on a throwaway server: 100 cycles, highest acknowledged 6,692, lost 0, partial 0, gaps 0, reopen failures 0. The `tls_` tests pass. 1,049 tests passed and 0 failed in all.

**A Postgres restart under a running server, which the kill test doesn't cover, as it works on the store alone.** `docker restart` of a throwaway Postgres 35 s into the 400-page rendition, at load about 6: the job stayed `running` for over 10 minutes and the workers logged "writer lease lost" about 17 times a second. After the server was restarted, the job finished in 72 s. A smaller run on this branch's binary and on one built from `main` before this spec gave the same on both: an upload to an existing vault answered 503 at 5, 30 and 60 s after Postgres came back, reads and new vaults worked, and after a server restart uploads answered 201. So this spec neither causes nor worsens it. The user approved landing with it open, and it's spec 089, in wave #90 before spec 030.

**After merging `main` with spec 043 (no conflicts), on the release server, at load about 13 to 140:** `pnpm` typecheck, lint, test, licenses, contrast, `e2e` and `budget` pass, and so do `e2e:search` 16 of 16, `e2e:wiki` 17 of 17, `e2e:shell` 17 of 17 and `e2e:documents` 11 of 11, on a throwaway Postgres with `en_5m.mp3`. In that run the 400-page document took 1 attempt and 77.4 s with no keep-alive, spec 042's item 5 opened in 819 to 837 ms, and a missing blob gave 410 `blob_missing` with the server's message on the screen. The user decided that a ceiling on a job's total time is a follow-up, spec 084.

**Follow-ups:** decide on a ceiling for a job's total time. Measure a scanned PDF that needs OCR on every page, now that only time matters, as it wasn't measured here or by spec 042.
