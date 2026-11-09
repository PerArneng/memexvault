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
(Filled in when the spec moves to done or dropped: what was built, decisions, numbers, follow-ups.)
