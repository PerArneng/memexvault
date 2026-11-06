---
id: 058
name: review-power-loss-durability
description: "Decide what memexvault should require or tell operators about power-loss safety, since Docker on macOS doesn't reach the drive"
---

# 058 review-power-loss-durability

> **On hold (2026-11-06):** Assumption in force, waiting for the user's review: docs explain power-loss safety, and startup doesn't measure flush time

## Goal
Have the user decide how far memexvault should go on power-loss safety, which it can't detect from inside Postgres.

## Background
**Depends on:** 006, whose Result has the numbers. A flush in Docker Desktop's VM takes 250 µs, and a real `F_FULLFSYNC` on the same machine 5 ms. So acknowledged commits in that setup survive process crashes, as 1,100 kill runs showed, but may not survive a power cut.

**Assumption in force:** Docker on macOS is for development. The server checks `fsync` and `synchronous_commit` as designed, and docs tell operators what to use: `fsync_writethrough` on native macOS, and `fdatasync` on Linux with a disk that honours flushes.

## Approach
Ask whether documentation is enough, or whether startup should also measure flush time and warn when it's implausibly fast.

## Verification
1. The user's answer is recorded here.
2. If a check is wanted, a follow-up spec adds it.

## Out of scope
Real power-cut testing.

## Result
(Filled in when the spec moves to done or dropped: what was built, decisions, numbers, follow-ups.)
