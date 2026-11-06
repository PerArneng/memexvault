---
id: 053
name: review-pdfium-isolation
description: Decide how strictly to sandbox pdfium, a C++ parser of untrusted PDFs, beyond the assumed worker process
---

# 053 review-pdfium-isolation

## Goal
Have the user confirm how isolated PDF parsing must be, since spec 002 chose a C++ parser for untrusted files.

## Background
**Depends on:** 002. pdfium is fuzzed heavily through Chromium, but in Chromium it runs in a sandbox. In-process, a parser crash stops the server and a memory bug could expose other vaults.

**Assumption in force:** spec 015 runs pdfium in child worker processes, the `memexvault` binary started with a worker subcommand. Crashes and hangs become failed jobs, and a global lock in pdfium doesn't limit throughput. No OS sandbox yet.

## Approach
Ask whether worker processes are enough, or whether to add a macOS `sandbox-exec` or Linux seccomp profile that denies network and file access to workers.

## Verification
1. The user's answer is recorded here.
2. If a sandbox is wanted, a test shows a worker can't open a file or a socket.

## Out of scope
Replacing pdfium.

## Result
Not started.
