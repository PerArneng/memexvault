---
id: 084
name: job-time-ceiling
description: "Give a running job a limit on its total time, so a hung handler can't hold its job until restart"
---

# 084 job-time-ceiling

**Depends on:** 082.

## Goal
A job whose handler hangs inside the server process ends with a clear failure within a set time, instead of holding its job until the server restarts. Long jobs that are still working aren't cut off.

## Background
Spec 082 made a worker renew a job's lease for as long as its handler runs. The keeper can't tell a stuck handler from a slow one, so a handler that hangs in-process, such as in the embedder, now holds its job until restart. Before spec 082 the lease ran out and another worker ran the job. Children have time limits and model calls have timeouts, so this needs a hang in code running in-process. It hasn't been seen, only reasoned about from the code in `run_one` in `crates/memexvault/src/workers.rs`.

## Approach
- A ceiling on total time per job kind, set from what each kind can legitimately take. Spec 082's Result lists the kinds and what bounds them today, and transcription can run for hours.
- At the ceiling the keeper stops renewing, so the fence refuses a late result and the job goes through the normal failure path with a reason that names the ceiling. An in-process thread can't be killed, so record what a truly hung thread still costs, and whether the worker slot comes back.
- Settings follow the existing `MEMEX_*` style, and each default is justified by a measured or bounded time.

## Verification
1. On ManualClock, a scripted handler that never returns is failed at its ceiling, with the reason recorded, and a late result writes nothing.
2. A second worker can claim and finish the job afterwards, and the hung handler doesn't stop other jobs of that kind. If it holds a worker slot, the number lost is recorded.
3. The 400-page PDF from spec 042 and a 60-minute recording from spec 027 still finish in 1 attempt, on a release build, with the load recorded.
4. A table of every job kind gives its ceiling and why.

## Out of scope
Killing threads, moving handlers into child processes, and changing `LEASE_MS`.

## Result
Built a ceiling on each attempt's time for every job kind. At the ceiling, the worker gives up on the handler and fails the attempt through the normal path, and the worker slot comes back at once. Items 1 to 4 pass. Migrations: none. New dependencies: none. `LEASE_MS` is unchanged.

**What was built.**
- `crates/memex-core/src/settings.rs`: `JobCeilings`, one per kind, read from `MEMEX_RENDITION_CEILING_S`, `MEMEX_TRANSCRIBE_CEILING_S`, `MEMEX_EMBED_CEILING_S`, `MEMEX_EXTRACT_CEILING_S`, `MEMEX_REVIEW_CEILING_S` and `MEMEX_AUDIT_CEILING_S`. A value must be at least 1. `for_kind` gives an unknown kind the largest ceiling.
- `crates/memexvault/src/workers.rs`: `run_one` now runs the handler on a thread of its own. The worker thread keeps the lease itself, checking every 250 ms, by the server's clock, so ManualClock drives it. The ceiling counts from the claim, not from when the thread started.
- At the ceiling, the worker stops renewing, marks the hold lost so a handler that checks it stops, and records the attempt as failed and retryable. The reason is "the job ran past its ceiling of {N} s for {kind} jobs, set by MEMEX_{X}_CEILING_S, so its worker gave up on it". The job then goes through the usual backoff and limit on attempts. The worker goes straight back to claiming.
- A handoff under a mutex decides whether a result arrived before the ceiling or after it, so a result is either recorded or thrown away, never both. A late result is also refused by the fence, as the attempt is no longer `running` under that owner.
- `abandoned_handlers()` counts the handler threads given up on that are still running. The server logs `still_running` when one is given up on, and again when it returns.
- `main.rs` gives each pool its kind's ceiling. `AGENTS.md` lists the settings.
- `specs/084-job-time-ceiling/tools/ceiling_run.py` runs item 3 on a real server, and shows the ceiling there with `--env`.

**What a truly hung thread costs.** The worker slot comes back at the ceiling, so **0 worker slots are lost.** What stays is the OS thread, its stack and whatever it holds: memory, a child process if it started one, and a connection if it hung inside a store call. It holds nothing in the job's row once its attempt is failed. The thread ends when the hang does, and then its result is thrown away. Shutdown no longer waits for it. Each hang adds one thread, so a handler that hangs on every attempt costs at most one thread per attempt, which is 5 before the job is dead. The real handlers checked here aren't truly hung at the ceiling: they see the lost hold and stop, as below.

**Measured.** All on release. Load averages are the 1, 5 and 15 minute figures. The machine was shared with five other workers.

1. **Passed,** in `a_handler_that_never_returns_is_failed_at_its_ceiling_and_its_late_result_writes_nothing`, on ManualClock and MemStore, with the default rendition ceiling of 14,400 s.
   - A handler that never returns until released is still running and held 1 s short of the ceiling, and a second worker's claim gets nothing.
   - At 14,400 s, `run_one` returns while the handler still hangs. The attempt's error is the reason above, naming `MEMEX_RENDITION_CEILING_S`.
   - Once released, the handler's late result, a page, isn't written. The vault's head is the same as before, and the job's row doesn't change.
2. **Passed,** in the same test and in `a_hung_handler_does_not_stop_other_jobs_of_its_kind`. Twenty repeated runs of the worker tests passed, in 2.5 to 2.6 s each.
   - After the backoff, a second worker claims the job and finishes it, `done` in 2 attempts.
   - A pool of 1 worker with 3 jobs and a 600 s ceiling: the hung handler holds the other 2 jobs `queued` until the ceiling. Then the same worker runs both to `done`, and the hung job goes back to the queue with "ceiling" in its first attempt's error. Worker slots lost: 0, as above.
   - On Postgres, `pg_job_lease_renewal_cancel_and_takeover` and `pg_job_lease_a_killed_process_frees_its_job_within_one_lease` pass on the new keeper, in 123.9 s, at load 16.8 to 10.5.
3. **Passed: both `done` in 1 attempt,** with the default ceilings, on the release binary of the verified head, uploaded together to one vault on a fresh database. Load was 27.6 / 26.7 / 18.6 at the start, 22.4 / 28.2 / 22.6 when the PDF ended, and 4.5 / 4.5 / 4.7 at the end. The server's peak RSS was 2,255 MB, with 59 threads at the end. Nothing in the server log was a warning or an error, other than the index steps' usual retries before a new vault's tables exist.
   - **The 400-page PDF from spec 042** (824,487 bytes, made by `web/apps/console/e2e-documents/make-pdf.ts`): 383.4 s, against a ceiling of 14,400 s.
   - **A 60-minute recording:** spec 027's corpus isn't on this machine, so I used its real English file instead: the Supreme Court argument 22-1008 (a US government work), downloaded from the address in spec 027's `corpus.md` and cut to its first 3,600 s with `ffmpeg -c copy`. It took 5,886.8 s, against a ceiling of 43,200 s, and was slowed by the load and the PDF beside it.
   - **First run, not counted:** with a stand-in recording made by looping a 29.4 s clip of speech 122 times, the PDF was `done` in 1 attempt, but the transcriber child died with SIGABRT 3 times. Apport recorded `memexvault transcribe-worker` with signal 6, and its stderr was empty. The job carried on from its saved progress each time, and was still running when the tool's 3,600 s limit ran out. That run was on the head before merging specs 086 and 087, at load 11 to 32. The ceiling played no part: it's 12 h, and no attempt was failed. The real recording had no crash.
   - **The ceiling on a real server,** with `MEMEX_RENDITION_CEILING_S=30` on the same PDF, at load 1.1 to 2.4: 5 attempts of 30.02 to 30.10 s, each with the reason above, then `dead` 185.7 s after upload. Each time, the rendition handler saw the lost hold and returned 35 to 41 ms after the worker gave up, and `still_running` went back to 0. So no thread was left behind.
4. **The table.** "Bounded by" is what limits a legitimate attempt today, from spec 082's Result and each kind's settings. Each ceiling is set to sit well above that bound, so slow but working jobs aren't cut off.

| Kind | Ceiling | Setting | Bounded by | Why this ceiling |
|---|---|---|---|---|
| `rendition` | 4 h (14,400 s) | `MEMEX_RENDITION_CEILING_S` | Parser 300 s, and 120 s for each OCR page. | 400 pages took 91.6 s at load about 160 in spec 082, and 383.4 s here at load about 25 beside a transcription. That's under 1 s a page, so 4 h covers about 15,000 pages at this run's rate, or a scan needing OCR on every page at 2 s a page for about 7,000 pages. A scanned PDF wasn't measured. |
| `transcription` | 12 h, twice `MEMEX_TRANSCRIBE_MAX_SECONDS` | `MEMEX_TRANSCRIBE_CEILING_S` | Recordings up to `MEMEX_TRANSCRIBE_MAX_SECONDS`, 6 h, with limits on stalls and memory. | 60 minutes took 832 s in spec 028, and 5,886.8 s here at load about 25. That's 1.6 times real time under heavy load. So a 6 h recording needs about 10 h at that rate, and twice its length leaves room. It follows the setting, so a longer limit raises it. |
| `embed` | 600 s | `MEMEX_EMBED_CEILING_S` | At most 32 texts a job on `multilingual-e5-small`. | Normally a few seconds. 600 s is about 100 times that, and it's the kind most likely to hang in-process. |
| `extract` | 4 h | `MEMEX_EXTRACT_CEILING_S` | Calls of up to 120 s each (`MEMEX_EXTRACT_TIMEOUT_S`) plus a retry, one batch after another, within the token budget. | Covers over 100 calls at their timeout, and a new attempt resumes from committed batches. |
| `review` | 900 s | `MEMEX_REVIEW_CEILING_S` | One call of up to 60 s plus a retry. | 7 times the worst legitimate time. |
| `audit` | 4 h | `MEMEX_AUDIT_CEILING_S` | A scan of the vault, then calls of up to 120 s each, within the schedule's budget. | The scan had a 15-minute hold before spec 082, which leaves 3.75 h for over 100 calls, and a new attempt resumes. |

**Decisions.**
- **The handler runs on a detached thread, and the worker keeps the lease.** Before, a keeper thread ran beside a handler on the worker's thread, so a hung handler held the slot. Now the slot comes back at the ceiling and only the thread is lost, and shutdown doesn't wait for it. The cost is one thread per running job, as before, and the handler and server must be `'static`, so callers pass `Arc`s.
- **The ceiling is per attempt and counts from the claim.** A total over all attempts would cut off a job that is retried after a crash, and the limit on attempts already bounds the total, to 5 times the ceiling.
- **A ceiling failure is retryable,** like a lost lease, since a hang may not happen again. A job that hangs every time goes `dead` after 5 attempts, as the demonstration shows.
- **Transcription's ceiling follows `MEMEX_TRANSCRIBE_MAX_SECONDS`,** so raising the longest recording allowed doesn't silently cut off long jobs.

**Definition of done,** on 9b70c21, which is this spec merged with `main` at b3728c4 (specs 086, 087 and 091): `cargo build --workspace` passes, and `cargo test --workspace` passes, with **1,003 passed, 0 failed and 112 ignored,** in 527 s at load 0.4 to 3.2. Clippy with `-D warnings` and `cargo fmt --check` pass. Before that merge, on 768bc9a, the same tests gave 996 passed and 0 failed, at load 11 to 18.
- **Postgres:** on a freshly dropped and recreated database of its own, `cargo test -p memexvault --release --no-fail-fast -- --ignored --test-threads=1 pg_` passes, **52 passed, 0 failed,** in 614 s at load 1.2 to 1.4. That includes `pg_kill_jobs_200_times`, `pg_kill_memexvault_1000_times` and both `pg_job_lease_` tests.
- **Skipped from that run:** `pg_child`, `pg_job_child` and `pg_job_child_long`, which are helpers the kill harnesses start and fail when run on their own. Also skipped: `pg_kill_postgres_100_times`, which kills a container; `pg_bad_cases`, which terminates other connections on the shared server; and `pg_full_disk`, which needs a small disk.

**Not run.**
- **`cargo deny check licenses`:** `cargo-deny` isn't installed on this machine. The branch changes no `Cargo.toml` or `Cargo.lock`.
- **The `web/` checks and the e2e checks:** this spec changes nothing under `web/`.
- **`meeting_eval` and `pdf_pipeline` with real models:** they need spec 027's corpus and `MEMEX_TEST_MODELS`. They now call `run_one` with their kind's default ceiling, and they compile and pass clippy.
- **Live runs:** none, as the brief says.

**Follow-ups.**
- **The transcriber aborted 3 times on a looped 29 s clip** (SIGABRT, with empty stderr), though it resumed each time. The real recording didn't. Worth finding out whether whisper.cpp asserts on repeated audio, and why the abort leaves nothing on stderr.
- `abandoned_handlers()` is only logged. A metric or a field in the health route would let an operator see threads given up on.
- Measure a scanned PDF that needs OCR on every page, to replace the estimate in the rendition row.
