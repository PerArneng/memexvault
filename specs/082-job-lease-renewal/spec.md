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
(Filled in when the spec moves to done or dropped: what was built, decisions, numbers, follow-ups.)
