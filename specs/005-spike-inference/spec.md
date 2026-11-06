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
**Decision: `intfloat/multilingual-e5-small` (384 dimensions) on ONNX Runtime through `ort` =2.0.0-rc.13, CPU, shared with OCR. Reranking is optional: `mmarco-mMiniLMv2-L12-H384-v1` int8, 20 candidates, 128 tokens.** The rerank-50 target failed for every model, so it's revised to rerank-20, with spec 055 for the user's review.

Reference machine, CPU, 434 Wikipedia topics in four languages (PagePile 68383). A query is the Wikidata description and the target is the topic's Wikipedia lead, so nDCG is relative to this small set. Full output is in `results.txt`, scripts in `tools/`.

| Embedder | Dim | Query p95, 4 threads / 1 thread | Passages/s, 8 threads, batch 32 | RSS | Size | nDCG@10 same language / cross |
|---|---|---|---|---|---|---|
| `paraphrase-multilingual-MiniLM-L12-v2` int8 | 384 | 5.6 / 5.4 ms | 83 | 0.57 GB | 118 MB | 0.804 / 0.784 |
| same, f32 | 384 | 9.4 / 13.7 ms | 40 | 1.22 GB | 470 MB | 0.805 / 0.783 |
| **`multilingual-e5-small` f32** | 384 | **9.4 / 15.5 ms** | 40 (92 at 128 tokens) | 1.41 GB | 470 MB | **0.914 / 0.832** |
| same, my int8 quantization | 384 | 5.8 / 6.1 ms | 82 | 0.60 GB | 118 MB | 0.899 / 0.802 |
| `multilingual-e5-base` f32 | 768 | **25.3** / 56.1 ms, misses the target | 12 | 2.64 GB | 1,110 MB | 0.926 / 0.881 |

e5-small per language, same-language: en 0.936, sv 0.897, de 0.913, es 0.911. e5-base is better but misses 20 ms on 4 threads and is 3 times slower to ingest, so it's out. int8 is the fallback if memory or ingest speed matters more than 1.5 to 3 points of quality. Changing later means re-embedding, which spec 019's generations already allow.

| Reranker | Inputs | Candidates | p95, 8 threads | Same-language effect | Cross-language effect |
|---|---|---|---|---|---|
| `mmarco` int8 | 256 tokens | 50 | 627 ms | small gain, es lost 0.011 | **loss** of 0.015 to 0.037 |
| `mmarco` f32 | 256 | 50 | 1,381 ms | as above | **loss** |
| `bge-reranker-v2-m3` int8 | 256 | 50 | 4,972 ms | gain 0.012 to 0.046 | mixed: sv→en lost 0.024, others gained |
| `mmarco` int8 | 128 | 50 | 287 ms, misses | sv 0.908→0.947, de 0.913→0.919 | sv→en 0.755→0.819 |
| **`mmarco` int8** | **128** | **20** | **133 ms** | sv 0.908→0.935, de 0.913→0.911 | sv→en 0.755→0.830 |
| `bge` int8 | 128 | 10 | 495 ms | sv →0.969, de →0.942 | sv→en →0.779 |
| `bge` int8 | 64 | 10 | 265 ms, misses | sv →0.922, de →0.924 | sv→en →0.800 |

The first three rows sit on e5-base's top 50 over 200 queries per pair. The others sit on e5-small's results over 40 queries for three pairs, so differences of about 0.02 are noise. In short, reranking helps modestly and depends on the case, so it stays opt-in. `bge` has the best same-language gain but can't meet any latency target on CPU.

**Rejected or not measured.** CoreML: e5-small query p95 40 ms against 9.4 ms on CPU, 5.6 passages/s, and `mmarco` f32 about 190 s per query, so the crate shouldn't enable it. `fastembed`: not measured, since it's a wrapper over `ort` whose convenience is the runtime model download we don't want. `rten` and `candle`: not measured, because `ort` passed and OCR already requires it, so one runtime costs nothing more. This means the table covers one runtime, a deliberate narrowing of Verification 1. `jina-reranker-v2-base-multilingual`: not run, CC-BY-NC.

**Verification.**
1. Tables above, for one runtime as explained.
2. Embedding p95 9.4 ms on 4 threads and 15.5 ms on 1, target ≤20 ms: pass. Rerank-50 p95 ≤250 ms: **fail for every model**. Rerank-20 at 128 tokens is 133 ms, which is the revised target in force, with spec 055 raised.
3. `sandbox-exec` denying network, with `curl` confirmed blocked: e5-small query p95 8.0 ms and reranking worked.
4. `otool -L`: `libSystem`, `libiconv`, `libc++`, and the Foundation, CoreML, CoreFoundation and objc frameworks. All are system libraries. Binary 30.7 MB with static ONNX Runtime.
5. Licences: e5-small MIT, `mmarco` Apache-2.0, MiniLM Apache-2.0, `bge` Apache-2.0 upstream through an ONNX community conversion.
6. Dimension 384. Spec 004 is still to run, and runs this model.
7. `docs/architecture.md` updated.

**`models fetch` layout.** A manifest compiled into the binary lists, per model: name, Hugging Face repo, pinned commit, file paths, size and sha256. Files go to `<models_dir>/<name>@<commit-prefix>/`, written to a temp name, hash-checked, then renamed. At startup the server checks that each file exists with the right size, and fails with a message naming `models fetch` if not, with no download. A full hash runs in `models fetch` and in `doctor`, not at each startup, since hashing 470 MB would eat the restart budget.

| Model | Commit | File | sha256 |
|---|---|---|---|
| `intfloat/multilingual-e5-small` | `614241f622f5` | `onnx/model.onnx` | `ca456c06b3a9…` |
| same | | `tokenizer.json` | `0b44a9d7b51c…` |
| `cross-encoder/mmarco-mMiniLMv2-L12-H384-v1` | `1427fd652930` | `onnx/model_qint8_arm64.onnx` | `1825907d6c1a…` |
| same | | `tokenizer.json` | `62c24cdc13d4…` |

The int8 file is arm64-specific. A Linux x86 build needs the `avx512_vnni` file with its own hash, or another variant, which spec 008 or a packaging spec should check.

**Details for spec 019.** e5 needs the prefixes `query: ` and `passage: `, mean pooling and normalization. Tokenization and pooling are ours, not provided by the runtime. Segments are 100 to 400 tokens and e5-small allows up to 512, but we tested 256, so spec 019 should truncate at 256 unless it measures otherwise.

**Follow-ups.** Spec 055. Ingest of 40 passages/s means about 83 minutes for 200k segments and 7 hours for 1M at 256 tokens on this machine, which is a background cost for operators to know about, and is recorded here since no target covers it.
