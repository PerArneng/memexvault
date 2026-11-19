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
Done. Everything below ran on 2026-11-19 against a pgvector 0.8.7 / Postgres 18 container of this worktree's own (podman, `127.0.0.1:55430`), with
scripted agents and the fake embedder, so no LLM or model was involved.

**What was built.**

- **Erasure** (`memex-core/src/erasure.rs`, `server/erasure.rs`). `Op::EraseSource` must be a ChangeSet's only op, counts as a delete, so it always
  waits for a person whoever proposes it, and can't be reverted. `POST /vaults/{id}/sources/{sid}/erasure` proposes and `GET` previews. The plan
  takes every version of the document and is ids only: renditions, segments, Derivations, extraction rows, evidence citing it, claims with no
  other evidence, entities left with no claim, edges to what goes (a page's own links stay and read as broken), jobs about it, and audit checks,
  planted tags and reviewer verdicts of what goes. ChangeSets lose the ops, evidence and mutations that came from it, and their validator details;
  entities are worked out again from the claims left; embedding jobs keep the rest of their batch; chat runs' Derivations lose the segment ids.
  Claims standing on other evidence too, and pages citing what went, are flagged. `GET /vaults/{id}/erasures` lists each erasure's record.
- **The log stays replayable.** The same pure rewrite functions rewrite every event's payload, keeping its envelope, in one transaction with a
  `source.erased` event (ids only), whose reducer removes what's still there and applies the same rewrites, so the live tables and a replay of the
  rewritten log arrive at the same state. A payload whose kind can't hold what's left becomes `{"erased": true, "mutations": [...]}`, which the
  reducer, upcaster, projectors and activity frames read as is. `Store::commit_rewriting` is the new trait method, with contract tests on both
  stores. `apply_as` refuses both forms, so only an approved erasure writes them.
- **The physical cascade** is an `erase` job of steps, which whole-vault deletion now runs too: chat threads the document reached
  (`erase_chat_threads_citing`), `LexicalIndex::erase` (tantivy merges the live generation into one segment, collects its files, and drops every
  other generation), `VectorIndex::erase` (every generation and the stored embeddings by content hash), unreferenced blobs, the cache generation
  (also bumped at commit, which resyncs live streams), and `Store::compact`, now `VACUUM FULL` and `ANALYZE` of every table in the vault's schema.
  The job waits until both projectors have applied the event and no rebuild is running. `drop_vault` sets a 10 s lock timeout.
- **Backup and restore** (`memex-core/src/backup.rs`, `memexvault backup|restore`): a record stream of the events, the blobs rows refer to, and the
  stored embeddings, from one snapshot and with no lease, with a header (head, canonical dump hash) and a SHA-256 trailer. Restore checks the whole
  archive first (trailer, every blob against its name, event order), then streams blobs into the store, replays 500 events a transaction, and
  refuses unless the tables hash to the header's dump. It restores under the vault's own id, which must be free, or `--new-id`, and overwrites
  only with `--replace`. A failed restore removes what it made.
- **Reconcile** (`POST /vaults/{id}/reconcile`, `memexvault reconcile`): tables against the log replayed in memory by dump hash, naming differing
  rows; `at` replays to a given event; blobs referenced against stored (`deep` re-hashes); each index's count and id digest against the store's;
  `repair` rebuilds a differing index. **Reindex** at `POST /vaults/{id}/reindex`. **The orphan sweep** on a schedule
  (`MEMEX_SWEEP_INTERVAL_S`, `MEMEX_SWEEP_GRACE_S`) and at `POST /vaults/{id}/sweep`. **Status** at `GET /api/v1/status` and Prometheus text at
  `GET /api/v1/metrics`: head, index lag and builds, open jobs by kind, dead jobs, review backlog, blob count and bytes, cache tiers.

**Verification.** `crates/memexvault/tests/ops.rs` on Postgres, and `crates/memex-core/tests/erasure.rs` in memory.

1. Backup during ingest: a thread uploaded documents throughout, 270 of them. The backup was taken at event 160 while the log reached 333: 161
   events, 103 blobs, 20 embeddings, 263,808 bytes, in 231 ms. Restored into another database and folder in 399 ms, under the same id. Reconcile
   there reported no differences (tables, log, blobs, `deep`), and reconcile of the original `--at 160` gave the same tables hash,
   `829b3e9c…`. Passes.
2. With 2,000 documents (2,000 segments), deleting the keyword index's folder and starting a new server rebuilt it in 89 ms over 11 projector
   steps (16 ms at 40 documents). All 6 queries returned the same 3,667 hits with the same scores to 4 decimals, compared with the old index
   merged. 4 of 6 came in exactly the same order; in the other 2, hits whose scores differ in the 5th decimal swapped, as summing BM25 over one
   segment or several differs in the last bits. Unmerged, the followed index matched on 5 of 6, as tantivy counts the old copies of updated
   documents in its statistics until a merge. Passes, with that noted.
3. A unique marker in a document, reaching its claims, a Person, a chat thread and a page's citation. Before: the marker was in 1 file of the
   vault folder (its blob), 10 terms of tantivy's term dictionaries (read through tantivy, as they're compressed and the text isn't stored), and 13
   Postgres data files (every column set to uncompressed storage in the test, as compression could only hide it from a byte scan). After the
   erasure and its job, with `VACUUM FULL` and a checkpoint: 0 files in the vault folder, 0 index terms, 0 files no index segment owns, 0 files
   under the database's `base/` and `global/`, 0 keyword hits, no search item holding it, and the source and a removed claim's lineage 404. The
   WAL still held it, in 2 files: the documented limit, with backups taken before. Passes.
4. The cited claim, its entity and its page reference: the claim and the Person went, the claim on both documents stayed and was flagged with its
   other evidence, the page was flagged. The rewritten log replayed into an empty store gave the live dump hash, on Postgres and in memory, and
   reconcile afterwards was clean. Passes.
5. A flipped byte, a cut file and a file that isn't a backup were each refused before anything was written, and no vault was created. Restoring
   over an existing vault without `--replace` was refused; with it, it worked. An agent's erasure proposal stayed pending, the agent couldn't
   approve it, and a person's own proposal waited too. `VACUUM FULL` of the erased vault's 34 tables took 139 ms in all, each table's lock at most
   8.5 ms (`segments`), on a small vault: lock time grows with a table's size, so on a large vault a reader of one table waits for that table's
   rewrite. Approval with the log rewrite took 32 ms, and the job with compaction 210 ms. Passes.
6. The preview listed exactly the removed, rewritten and flagged rows the erasure's record then held, and every removed key was gone. Passes.

Also: `cargo build`, `test`, `clippy -D warnings`, `fmt --check` and `deny check licenses` on the workspace, and the Postgres `pg_` suite.

**Decisions.** An erasure takes every version of a document, as versions share their text. An entity left with no claim goes, as what's left of
it came with them. Pages are never edited, only flagged. The erase job's input keeps the erased blobs' and texts' hashes, which can confirm a known
file was there and can't give it back. Chat threads aren't in a backup, as they're off the log by design.

**Limits and follow-ups.** WAL and earlier backups keep what they kept. Reconcile and the erasure's log rewrite each read the whole log, and
reconcile holds the vault twice in memory, which wants measuring at spec 033's scale. Status lists every blob on each call. The sweep re-reads
references before deleting, which leaves a small window for a commit referring to an old orphan's bytes. The console has no screens for these
routes yet.
