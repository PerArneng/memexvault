---
id: 005
name: spike-inference
description: Choose the local embedding and reranking models and runtime with no network or system libraries
---

# 005 spike-inference

## Goal
Pick the models and runtime for `memex-inference`. Pass on the reference machine with the network off: query embedding
p95 ≤20 ms, rerank of 50 passages p95 ≤250 ms, and system libraries only.

## Background
**Depends on:** none, but read the runtime recorded by spec 003 if it has finished. Design: `docs/architecture.md`,
sections Decisions, Targets.

Candidates: `fastembed` 7.6 (uses `ort`), `ort` 2.0 release candidate directly, `rten` 0.27 and `candle` 0.11. Risks to
test: runtime model downloads would break the no-external-systems rule, `ort` may load a dynamic library, and `ort` is
still a release candidate. One ML runtime shared with OCR is preferred if it costs little.

## Approach
- Throwaway crate in this spec's folder.
- At least two multilingual embedding models and two multilingual cross-encoders, on CPU and on any Apple acceleration available without extra
  system installs.
- Measure latency at batch 1 and batch 32, ingest throughput, RSS, binary size, model size and model licence.
- Measure retrieval quality on a small public labelled set (nDCG@10), so speed isn't chosen alone. Cover English, Swedish, German
  and Spanish, plus queries in one language that must find text in another.
- Design the `models fetch` layout: pinned URL, hash and an offline check at startup.

## Verification
1. Results table per runtime and model.
2. Chosen embedding p95 ≤20 ms and rerank-50 p95 ≤250 ms.
3. Run with networking blocked by the macOS sandbox (`sandbox-exec`, deny network) and model files on disk: all inference
   works.
4. `otool -L` shows system libraries only.
5. Model licences recorded.
6. The vector dimension is recorded, so spec 004's matching results apply. If spec 004 has finished, confirm it tested
   that dimension.
7. `docs/architecture.md` records runtime, models and quality numbers per language, and cross-language.

## Out of scope
The projector, query caches, GPU servers and remote embedding APIs.

## Result
Not started.
