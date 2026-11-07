//! The activity stream on the facade: who may follow a vault, what the log holds, and where each document stands. Streams hold no connection while they wait, so these are all short calls.
use super::{Server, ServerEdges, ServerError};
use crate::activity::{self, ActivityLimits, DocumentStage, EmbedState, Progress, StageInputs};
use crate::auth::{Caller, Operation};
use crate::{PAGE_LIMIT, RENDITION_JOB, Snapshot, Store, TRANSCRIBE_JOB, VectorIndex};
use crate::{CacheMarks, LexicalIndex};
use memex_types::{DocumentFacts, Event, Generation, JobRow, JobState, ObjectId, ProgressEntry, ProgressUpdate, Row, SourceRow, Stage, Table, VaultId};
use std::collections::HashMap;

/// How many unfinished, dead or cancelled parse jobs of a kind and state a snapshot looks through. Past this, a document can show as stored while its job is further on.
const JOB_SCAN: usize = PAGE_LIMIT;
/// How many (document, embedding job) pairs a snapshot looks through.
const EMBED_SCAN: usize = 5000;
/// How many pages of sources a filtered snapshot reads before it answers with what it has and a cursor.
const SCAN_PAGES: usize = 20;

/// One document on the live board.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentActivity {
    pub source: SourceRow,
    pub facts: DocumentFacts,
    pub stage: DocumentStage,
    pub progress: Option<ProgressEntry>,
    pub waiting_since_ms: Option<i64>,
    pub next_run_at_ms: Option<i64>,
}

/// A page of the live board, and the log position it reflects, which a stream can follow from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActivitySnapshot {
    pub head: Option<u64>,
    pub documents: Vec<DocumentActivity>,
    /// Where to carry on when a filter read as many pages as one request may. A full answer's cursor is the caller's to work out, by asking for one more than it wants.
    pub next_cursor: Option<ObjectId>,
}

fn source_of(job: &JobRow) -> Option<ObjectId> {
    job.input.get("source_id").and_then(|v| v.as_str()).map(|s| ObjectId(s.into()))
}

fn merged(a: EmbedState, b: EmbedState) -> EmbedState {
    let rank = |s| match s {
        EmbedState::Clear => 0,
        EmbedState::Queued => 1,
        EmbedState::Running => 2,
        EmbedState::Dead => 3,
    };
    if rank(a) >= rank(b) { a } else { b }
}

impl<E: ServerEdges> Server<E> {
    /// Sizes and intervals, set once at startup from settings.
    pub fn set_activity_limits(&self, limits: ActivityLimits) {
        *self.activity_limits.lock().expect("lock") = limits;
    }

    pub fn activity_limits(&self) -> ActivityLimits {
        *self.activity_limits.lock().expect("lock")
    }

    /// Whether this caller may follow this vault. It runs when a stream opens and again for as long as it stays open.
    pub fn activity_allowed(&self, caller: &Caller, vault: &VaultId) -> Result<(), ServerError> {
        self.allow(caller, Operation::VaultContentRead(vault))?;
        self.vault_exists(vault)
    }

    /// The log's head and the cache generation, in one statement with no snapshot, so looking costs little when nothing has happened.
    pub fn activity_marks(&self, vault: &VaultId) -> Result<CacheMarks, crate::StoreError> {
        self.engine.store().cache_marks(vault)
    }

    /// A page of the log for a stream whose caller has been checked. The caller of this keeps it to frames.
    pub fn activity_events(&self, vault: &VaultId, after: Option<u64>) -> Result<Vec<Event>, ServerError> {
        Ok(self.engine.store().snapshot(vault)?.events(after)?)
    }

    /// Reports from workers since a version, and the version to ask from next.
    pub fn activity_progress(&self, vault: &VaultId, version: u64) -> (Vec<ProgressEntry>, u64) {
        self.progress_board.since(vault, version)
    }

    /// The time of the event the slowest index has reached, or none when both have reached the head.
    fn indexed_to_ms<S: Snapshot>(&self, snap: &S, vault: &VaultId, head: Option<u64>) -> Result<Option<i64>, ServerError> {
        let lexical = LexicalIndex::watermark(&self.lexical, vault, Generation::Live)?;
        let vector = VectorIndex::watermark(&self.vector, vault, Generation::Live)?;
        let slowest = match (lexical, vector) {
            (Some(l), Some(v)) => Some(l.min(v)),
            _ => None,
        };
        match (slowest, head) {
            (_, None) => Ok(None),
            (Some(reached), Some(h)) if reached >= h => Ok(None),
            // Nothing is indexed yet, so everything is ahead of it.
            (None, Some(_)) => Ok(Some(i64::MIN)),
            (Some(reached), Some(_)) => Ok(Some(snap.event_time(reached)?.unwrap_or(i64::MIN))),
        }
    }

    /// The live board: each document's stage, in id order. `stage` keeps only documents in that stage, reading further pages to fill the answer, up to a limit.
    pub fn activity_documents(
        &self,
        caller: &Caller,
        vault: &VaultId,
        stage: Option<Stage>,
        after: Option<&ObjectId>,
        limit: usize,
    ) -> Result<ActivitySnapshot, ServerError> {
        self.activity_allowed(caller, vault)?;
        let snap = self.engine.store().snapshot(vault)?;
        let head = snap.head()?;
        let mut parse_jobs: HashMap<ObjectId, JobRow> = HashMap::new();
        for kind in [RENDITION_JOB, TRANSCRIBE_JOB] {
            for state in [JobState::Queued, JobState::Running, JobState::Dead, JobState::Cancelled] {
                for job in snap.jobs(Some(kind), Some(state), None, JOB_SCAN)? {
                    if let Some(source) = source_of(&job) {
                        // Ids go by time, so the newest job for a document wins.
                        match parse_jobs.get(&source) {
                            Some(have) if have.header.id > job.header.id => {}
                            _ => {
                                parse_jobs.insert(source, job);
                            }
                        }
                    }
                }
            }
        }
        let mut embeds: HashMap<ObjectId, EmbedState> = HashMap::new();
        for (source, state) in snap.open_embed_sources(EMBED_SCAN)? {
            let new = match state {
                JobState::Queued => EmbedState::Queued,
                JobState::Running => EmbedState::Running,
                JobState::Dead => EmbedState::Dead,
                JobState::Done | JobState::Cancelled => EmbedState::Clear,
            };
            let slot = embeds.entry(source).or_default();
            *slot = merged(*slot, new);
        }
        let indexed_to_ms = self.indexed_to_ms(&snap, vault, head)?;
        let board = self.progress_board.current(vault);
        let mut documents = vec![];
        let mut cursor = after.cloned();
        for _ in 0..SCAN_PAGES {
            let page = snap.rows(Table::Source, cursor.as_ref())?;
            let Some(last) = page.last().map(|r| r.header().id.clone()) else {
                return Ok(ActivitySnapshot {
                    head,
                    documents,
                    next_cursor: None,
                });
            };
            // Without a filter there's no need to read further than the answer.
            let wanted = if stage.is_some() { PAGE_LIMIT } else { limit - documents.len() };
            let sources: Vec<SourceRow> = page
                .into_iter()
                .filter_map(|r| if let Row::Source(s) = r { Some(s) } else { None })
                .take(wanted)
                .collect();
            let ids: Vec<ObjectId> = sources.iter().map(|s| s.header.id.clone()).collect();
            let facts = snap.document_facts(&ids)?;
            for (source, facts) in sources.into_iter().zip(facts) {
                let parse_job = parse_jobs.get(&source.header.id);
                let progress = parse_job
                    .or(facts.extraction_job.as_ref())
                    .and_then(|j| board.iter().find(|e| e.update.job_id == j.header.id))
                    .cloned();
                let placed = activity::document_stage(StageInputs {
                    facts: &facts,
                    parse_job,
                    embed: embeds.get(&source.header.id).copied().unwrap_or_default(),
                    indexed_to_ms,
                    progress: progress.as_ref().map(|e| &e.update),
                });
                if stage.is_some_and(|s| s != placed.stage) {
                    continue;
                }
                let holding = placed.job_id.as_ref().and_then(|id| {
                    parse_job
                        .filter(|j| &j.header.id == id)
                        .or(facts.extraction_job.as_ref().filter(|j| &j.header.id == id))
                });
                let (waiting_since_ms, next_run_at_ms) = holding.map_or((None, None), |j| {
                    (j.wait.as_ref().map(|w| w.since_ms), (j.state == JobState::Queued).then_some(j.next_run_at_ms))
                });
                documents.push(DocumentActivity {
                    source,
                    facts,
                    stage: placed,
                    progress,
                    waiting_since_ms,
                    next_run_at_ms,
                });
                if documents.len() == limit {
                    return Ok(ActivitySnapshot {
                        head,
                        documents,
                        next_cursor: None,
                    });
                }
            }
            cursor = Some(last);
        }
        // Read as many pages as one request may without filling the answer, so the caller carries on from here.
        Ok(ActivitySnapshot {
            head,
            documents,
            next_cursor: cursor,
        })
    }
}

impl<E: ServerEdges> Progress for Server<E> {
    fn report(&self, vault: &VaultId, update: ProgressUpdate) {
        self.progress_board.report(vault, update, self.now());
    }

    fn finished(&self, vault: &VaultId, job_id: &ObjectId) {
        self.progress_board.finished(vault, job_id);
    }
}
