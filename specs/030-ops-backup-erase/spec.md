---
id: 030
name: ops-backup-erase
description: Add backup, restore, reconcile, reindex and a verifiable erasure cascade
---

# 030 ops-backup-erase

## Goal
An operator can back up, restore, repair and reindex a vault with no tools beyond the server itself. Erasing a source removes its content
everywhere it spread, with zero hits for the purged text.

## Background
**Depends on:** 021, 026. Design: `docs/architecture.md`, sections Backends, Consistency.

## Approach
- A consistent per-vault export of the log and blobs into a portable archive, through the traits and without stopping reads.
  Import into an empty vault, plus a rebuild of indexes. Operators can use Postgres's own backup tools as well, and this
  format works without them.
- A reconcile command comparing log, tables, indexes and blobs, with counts and hashes and auto-repair for derived data.
- Reindex commands, and an orphan blob sweep schedule.
- Erasure as a proposal needing human approval. It walks lineage from the Source, removes payloads, blobs, renditions, embeddings
  and index entries, invalidates derived claims and pages, bumps the vault generation, compacts storage, and appends an `Erased`
  event with no content.
- Metrics and a status API: lag, queue depth, sizes and cache stats.
- Erasure gets an impact preview route listing what it would remove. The cascade is a list of steps, so spec 049 can add chat threads. Spec 049 has landed: a step must call `Server::erase_chat_threads_citing` after a source's rows are gone, then compact, and decide what to do with `segment_ids` on `chat_run` Derivations, which are ids and no text. Spec 049's item 6 ran only by calling that function directly. A route lists claims invalidated by erasure. Whole-vault deletion from spec 012 runs through the same cascade and drops the vault's schema, blobs and indexes.

- Erasure also covers what spec 019 stores: rows in `embeddings` for erased text, vectors in every generation, and the old generation kept for rollback. `compact` must include those tables, and `drop_vault` should set a lock timeout (spec 018).

## Verification
1. Backup during ingest, restore elsewhere, then reconcile reports no differences up to the backup's sequence.
2. Deleting an index directory and restarting rebuilds it with identical query results, and time is recorded.
3. Erasure test with a unique marker string in a document: 0 search hits, 0 API hits, and a raw byte scan of the whole vault folder
   and the Postgres data files finds no marker, after `VACUUM FULL`, a checkpoint, and a tantivy merge. If raw bytes survive, the
   spec fails until compaction deals with them. WAL and archives are checked and recorded as a documented limit.
4. Claims and pages depending on the erased source are flagged or removed, and the log still replays to the same state.
5. Bad cases: a corrupt backup is refused, restore doesn't overwrite an existing vault without an explicit flag, and an erasure proposal
   from an agent never auto-commits. The `VACUUM FULL` lock time is measured and recorded.
6. The impact preview lists exactly what the erasure then removes, on a fixture vault.

## Out of scope
Crypto-shredding, remote backup targets, replication and backups already taken before an erasure (documented as a limit).

## Result
Not started.
