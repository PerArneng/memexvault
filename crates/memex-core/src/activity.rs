//! The live view's rules, as pure functions on plain data: which stage a document is in, what a slow client's buffer does, where a reconnecting client starts, and the in-memory board workers report progress to.
use crate::extraction::{ExtractionState, extraction_state};
use crate::{EMBED_JOB, EXTRACT_JOB, RENDITION_JOB, REVIEW_JOB, TRANSCRIBE_JOB};
use memex_types::{DocumentFacts, JobRow, JobState, ObjectId, PageFlag, ProgressEntry, ProgressUpdate, Stage, StageStatus, VaultId};
use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

/// The tunable sizes and intervals of the activity stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActivityLimits {
    /// Open streams per principal, over all vaults.
    pub max_streams: usize,
    pub buffer: BufferLimits,
    /// Recent durable frames kept per vault, so most reconnects read nothing from the log.
    pub ring_frames: usize,
    /// How often the one reader of a vault's log looks for new events.
    pub poll_ms: u64,
    pub heartbeat_ms: u64,
    /// How often each credential holding a stream open is checked again.
    pub recheck_ms: u64,
    /// How long a credential may go unchecked because the store is away, before its streams end.
    pub unverified_ms: u64,
    /// How long a stream's socket may refuse writes before the connection is closed.
    pub stall_ms: u64,
}

impl Default for ActivityLimits {
    fn default() -> Self {
        Self {
            max_streams: 16,
            buffer: BufferLimits {
                max_frames: 1024,
                max_bytes: 1024 * 1024,
            },
            ring_frames: 4096,
            poll_ms: 200,
            heartbeat_ms: 15_000,
            recheck_ms: 1_000,
            unverified_ms: 10_000,
            stall_ms: 30_000,
        }
    }
}

/// How much one client may have waiting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferLimits {
    pub max_frames: usize,
    pub max_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameClass {
    /// From the log. Losing one means the client has to resync.
    Durable,
    /// In memory only, and the next report replaces it.
    Progress,
    /// Says the stream is alive, which anything already waiting says too.
    Heartbeat,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admission {
    Accept,
    /// Leave this frame out, and nothing else changes.
    Drop,
    /// Drop what's waiting and tell the client to resync.
    Overflow,
}

/// What a client's buffer does with a new frame. The half kept back for durable frames means progress can't cause a resync by itself.
pub fn admit(class: FrameClass, frames: usize, bytes: usize, frame_bytes: usize, limits: BufferLimits) -> Admission {
    match class {
        FrameClass::Heartbeat if frames > 0 => Admission::Drop,
        FrameClass::Heartbeat => Admission::Accept,
        FrameClass::Progress if frames + 1 > limits.max_frames / 2 || bytes + frame_bytes > limits.max_bytes / 2 => Admission::Drop,
        FrameClass::Progress => Admission::Accept,
        // A frame over the limit by itself still goes to an empty buffer, or it could never be delivered.
        FrameClass::Durable if frames > 0 && (frames + 1 > limits.max_frames || bytes + frame_bytes > limits.max_bytes) => Admission::Overflow,
        FrameClass::Durable => Admission::Accept,
    }
}

/// Why a client has to fetch a new snapshot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResyncReason {
    /// It read too slowly, and its buffer overflowed.
    Overflow,
    /// The position it asked for can't be read as one, or is ahead of the log.
    UnknownPosition,
    /// The log no longer holds what comes after that position.
    LogGap,
    /// Erasure moved the vault to a new cache generation, so what it had may be gone.
    Erased,
}

impl ResyncReason {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Overflow => "overflow",
            Self::UnknownPosition => "unknown_position",
            Self::LogGap => "log_gap",
            Self::Erased => "erased",
        }
    }
}

/// What a client asked for when it connected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumeRequest {
    /// Nothing, so it starts at the current position.
    Fresh,
    /// After this position, where -1 is before the first event.
    After(i64),
    /// Something that isn't a position.
    Malformed,
}

/// The position a client asks for. `Last-Event-ID`, which a browser sends itself when it reconnects, comes before a position named in the URL.
pub fn resume_request(last_event_id: Option<&str>, after: Option<&str>) -> ResumeRequest {
    match last_event_id.or(after) {
        None => ResumeRequest::Fresh,
        Some(text) => match text.trim().parse::<i64>() {
            Ok(n) if n >= -1 => ResumeRequest::After(n),
            _ => ResumeRequest::Malformed,
        },
    }
}

/// Where a new stream starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumePlan {
    /// Only what comes next. `named` says the client asked for this very position.
    Live {
        named: bool,
    },
    /// What the ring holds after this position.
    FromRing(i64),
    /// From the log, after the first position and up to the second, and then what comes next.
    FromLog {
        after: i64,
        until: i64,
    },
    Resync(ResyncReason),
}

/// Decides where a stream starts. `oldest` is the first sequence the ring holds, and `max_replay` is the most it may copy into a new client's buffer.
pub fn plan_resume(request: ResumeRequest, position: i64, oldest: Option<i64>, max_replay: usize) -> ResumePlan {
    let after = match request {
        ResumeRequest::Fresh => return ResumePlan::Live { named: false },
        ResumeRequest::Malformed => return ResumePlan::Resync(ResyncReason::UnknownPosition),
        ResumeRequest::After(n) => n,
    };
    if after > position {
        return ResumePlan::Resync(ResyncReason::UnknownPosition);
    }
    if after == position {
        return ResumePlan::Live { named: true };
    }
    match oldest {
        Some(first) if after + 1 >= first && (position - after) as u64 <= max_replay as u64 => ResumePlan::FromRing(after),
        _ => ResumePlan::FromLog { after, until: position },
    }
}

/// The index of the first sequence that doesn't follow the one before it, starting after `after`.
pub fn first_gap(after: i64, seqs: impl IntoIterator<Item = u64>) -> Option<usize> {
    let mut expected = after + 1;
    for (i, seq) in seqs.into_iter().enumerate() {
        if seq as i64 != expected {
            return Some(i);
        }
        expected += 1;
    }
    None
}

/// Takes the report of what a job is doing, and is told when the job has ended. Workers hold this and nothing about how a report travels.
pub trait Progress {
    fn report(&self, vault: &VaultId, update: ProgressUpdate);
    fn finished(&self, vault: &VaultId, job_id: &ObjectId);
}

/// The most jobs one vault's board tracks. A worker that never says it finished can't grow this.
pub const BOARD_ENTRIES: usize = 256;

#[derive(Default)]
struct VaultBoard {
    version: u64,
    entries: BTreeMap<ObjectId, (u64, ProgressEntry)>,
}

/// The latest report per running job, in memory, so a restart forgets it and nothing durable depends on it.
#[derive(Default)]
pub struct ProgressBoard {
    vaults: Mutex<HashMap<VaultId, VaultBoard>>,
}

impl ProgressBoard {
    pub fn report(&self, vault: &VaultId, update: ProgressUpdate, time_ms: i64) {
        let mut vaults = self.vaults.lock().expect("lock");
        let board = vaults.entry(vault.clone()).or_default();
        board.version += 1;
        let version = board.version;
        board.entries.insert(update.job_id.clone(), (version, ProgressEntry { update, time_ms }));
        if board.entries.len() > BOARD_ENTRIES
            && let Some(oldest) = board.entries.iter().min_by_key(|(_, (v, _))| *v).map(|(k, _)| k.clone())
        {
            board.entries.remove(&oldest);
        }
    }

    pub fn finished(&self, vault: &VaultId, job_id: &ObjectId) {
        if let Some(board) = self.vaults.lock().expect("lock").get_mut(vault) {
            board.entries.remove(job_id);
        }
    }

    /// What changed since a version, oldest report first, with the version to ask from next.
    pub fn since(&self, vault: &VaultId, version: u64) -> (Vec<ProgressEntry>, u64) {
        let vaults = self.vaults.lock().expect("lock");
        let Some(board) = vaults.get(vault) else { return (vec![], 0) };
        let mut changed: Vec<&(u64, ProgressEntry)> = board.entries.values().filter(|(v, _)| *v > version).collect();
        changed.sort_by_key(|(v, _)| *v);
        (changed.into_iter().map(|(_, e)| e.clone()).collect(), board.version)
    }

    pub fn current(&self, vault: &VaultId) -> Vec<ProgressEntry> {
        self.vaults
            .lock()
            .expect("lock")
            .get(vault)
            .map_or(vec![], |b| b.entries.values().map(|(_, e)| e.clone()).collect())
    }
}

/// Where a kind of job's work belongs. `None` for work that isn't about one document's way through the pipeline, such as an audit.
pub fn stage_of_job(kind: &str) -> Option<Stage> {
    match kind {
        RENDITION_JOB => Some(Stage::Parsing),
        TRANSCRIBE_JOB => Some(Stage::Transcription),
        EMBED_JOB => Some(Stage::Embedding),
        EXTRACT_JOB => Some(Stage::Extraction),
        REVIEW_JOB => Some(Stage::Review),
        _ => None,
    }
}

/// How a job is going, in the words the stage model uses.
pub fn status_of_job(job: &JobRow) -> StageStatus {
    match job.state {
        JobState::Queued if job.wait.is_some() => StageStatus::Waiting,
        JobState::Queued if job.attempts.iter().any(|a| a.error.is_some()) => StageStatus::Retrying,
        JobState::Queued => StageStatus::Queued,
        JobState::Running => StageStatus::Active,
        JobState::Done => StageStatus::Done,
        JobState::Dead | JobState::Cancelled => StageStatus::Failed,
    }
}

/// Whether a document's embedding jobs are holding it up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EmbedState {
    /// Nothing unfinished.
    #[default]
    Clear,
    Queued,
    Running,
    /// A job ran out of attempts, so part of the document can't be found by meaning.
    Dead,
}

/// Everything a document's stage depends on.
#[derive(Debug, Clone, Copy)]
pub struct StageInputs<'a> {
    pub facts: &'a DocumentFacts,
    /// The newest unfinished, dead or cancelled job turning the original into a rendition.
    pub parse_job: Option<&'a JobRow>,
    pub embed: EmbedState,
    /// The time of the event the slowest index has reached, or `None` when they have all reached the log's head.
    pub indexed_to_ms: Option<i64>,
    /// What the worker on this document last said.
    pub progress: Option<&'a ProgressUpdate>,
}

/// A document's place in the pipeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentStage {
    pub stage: Stage,
    pub status: StageStatus,
    /// The job this depends on now, if one does.
    pub job_id: Option<ObjectId>,
    pub attempts: u32,
    /// Some part of it didn't work, whatever stage it's in now.
    pub partial: bool,
}

fn rendition_is_partial(facts: &DocumentFacts) -> bool {
    facts.rendition.as_ref().is_some_and(|r| {
        r.media.as_ref().is_some_and(|m| m.skipped_spans > 0) || r.pages.iter().any(|p| p.flags.iter().any(|f| matches!(f, PageFlag::OcrFailed { .. })))
    })
}

/// A parse job that ended badly counts unless a newer rendition has come since, which means someone got past it.
fn parse_job_counts(job: &JobRow, facts: &DocumentFacts) -> bool {
    match job.state {
        JobState::Done => false,
        JobState::Dead | JobState::Cancelled => match (&facts.rendition, job.finished_at_ms) {
            (Some(r), Some(finished)) => r.created_at_ms <= finished,
            _ => true,
        },
        JobState::Queued | JobState::Running => true,
    }
}

/// Pure: which stage a document is in, from its job and event state. The earliest unfinished work decides, so a later stage can't hide an earlier failure.
pub fn document_stage(inputs: StageInputs<'_>) -> DocumentStage {
    let facts = inputs.facts;
    let mut partial = rendition_is_partial(facts);
    if let Some(job) = inputs.parse_job.filter(|j| parse_job_counts(j, facts)) {
        let status = status_of_job(job);
        let attempts = job.attempts.len() as u32;
        // Nothing has happened to it yet, so it's only stored.
        let untouched = status == StageStatus::Queued && attempts == 0;
        let stage = if untouched {
            Stage::Stored
        } else {
            let by_kind = stage_of_job(&job.kind).unwrap_or(Stage::Parsing);
            inputs
                .progress
                .filter(|p| job.state == JobState::Running && p.job_id == job.header.id)
                .map_or(by_kind, |p| p.stage)
        };
        return DocumentStage {
            stage,
            status,
            job_id: Some(job.header.id.clone()),
            attempts,
            partial,
        };
    }
    let Some(rendition) = &facts.rendition else {
        return DocumentStage {
            stage: Stage::Stored,
            status: StageStatus::Queued,
            job_id: None,
            attempts: 0,
            partial,
        };
    };
    let at = |stage, status| DocumentStage {
        stage,
        status,
        job_id: None,
        attempts: 0,
        partial,
    };
    if inputs.indexed_to_ms.is_some_and(|t| rendition.created_at_ms > t) {
        return at(Stage::Indexing, StageStatus::Active);
    }
    match inputs.embed {
        EmbedState::Clear => {}
        EmbedState::Queued => return at(Stage::Embedding, StageStatus::Queued),
        EmbedState::Running => return at(Stage::Embedding, StageStatus::Active),
        EmbedState::Dead => return at(Stage::Embedding, StageStatus::Failed),
    }
    if let (Some(row), Some(job)) = (&facts.extraction, &facts.extraction_job) {
        let state = extraction_state(row, Some(job));
        partial = partial || state == ExtractionState::Partial;
        if !matches!(state, ExtractionState::Done | ExtractionState::Partial | ExtractionState::NotStarted) {
            return DocumentStage {
                stage: Stage::Extraction,
                status: status_of_job(job),
                job_id: Some(job.header.id.clone()),
                attempts: job.attempts.len() as u32,
                partial,
            };
        }
    }
    if facts.pending_review > 0 {
        return DocumentStage {
            stage: Stage::Review,
            status: StageStatus::Active,
            job_id: None,
            attempts: 0,
            partial,
        };
    }
    DocumentStage {
        stage: Stage::Done,
        status: if partial { StageStatus::Partial } else { StageStatus::Done },
        job_id: None,
        attempts: 0,
        partial,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::jobs;
    use memex_types::{AgentUsage, ExtractionRow, ExtractionStop, Header, MediaFacts, PageOutcome, PageRoute, RenditionRow, Status};
    use serde_json::json;

    fn id(s: &str) -> ObjectId {
        ObjectId(s.into())
    }

    fn header(i: &str) -> Header {
        Header {
            id: id(i),
            creator: "c".into(),
            status: Status::Active,
            version: 1,
            acl_id: None,
        }
    }

    fn job(kind: &str) -> JobRow {
        jobs::new_job(id("j"), "c", kind, json!({}), "k".into(), 3, 100)
    }

    fn rendition(created: i64) -> RenditionRow {
        RenditionRow {
            header: header("r"),
            source_id: id("s"),
            parser: "p".into(),
            pipeline_version: 1,
            text_blob: "t".into(),
            words_blob: "w".into(),
            page_count: 1,
            segment_count: 1,
            created_at_ms: created,
            superseded_by: None,
            pages: vec![],
            media: None,
        }
    }

    fn facts() -> DocumentFacts {
        DocumentFacts {
            source_id: id("s"),
            rendition: None,
            extraction: None,
            extraction_job: None,
            pending_review: 0,
        }
    }

    fn with_rendition() -> DocumentFacts {
        DocumentFacts {
            rendition: Some(rendition(1000)),
            ..facts()
        }
    }

    fn extraction(stop: Option<ExtractionStop>, done: u32, total: u32) -> ExtractionRow {
        ExtractionRow {
            header: header("r"),
            source_id: id("s"),
            job_id: id("j"),
            model: None,
            prompt_hash: None,
            segments_total: total,
            segments_done: done,
            batches_failed: 0,
            proposed: 0,
            rejected: 0,
            usage: AgentUsage::default(),
            token_budget: 0,
            stop,
            updated_at_ms: 0,
        }
    }

    fn inputs(facts: &DocumentFacts) -> StageInputs<'_> {
        StageInputs {
            facts,
            parse_job: None,
            embed: EmbedState::Clear,
            indexed_to_ms: None,
            progress: None,
        }
    }

    fn pair(s: &DocumentStage) -> (Stage, StageStatus) {
        (s.stage, s.status)
    }

    #[test]
    fn an_upload_nothing_has_touched_is_stored() {
        let f = facts();
        let j = job(RENDITION_JOB);
        let s = document_stage(StageInputs {
            parse_job: Some(&j),
            ..inputs(&f)
        });
        assert_eq!(pair(&s), (Stage::Stored, StageStatus::Queued));
        assert_eq!(s.job_id, Some(id("j")));
        assert_eq!(pair(&document_stage(inputs(&f))), (Stage::Stored, StageStatus::Queued));
    }

    #[test]
    fn a_running_job_is_parsing_or_transcribing_and_progress_refines_it() {
        let f = facts();
        let j = jobs::claimed(job(RENDITION_JOB), "w", 200);
        assert_eq!(
            pair(&document_stage(StageInputs {
                parse_job: Some(&j),
                ..inputs(&f)
            })),
            (Stage::Parsing, StageStatus::Active)
        );
        let ocr = ProgressUpdate {
            job_id: id("j"),
            source_id: Some(id("s")),
            stage: Stage::Ocr,
            done: 12,
            total: Some(400),
        };
        let s = document_stage(StageInputs {
            parse_job: Some(&j),
            progress: Some(&ocr),
            ..inputs(&f)
        });
        assert_eq!(pair(&s), (Stage::Ocr, StageStatus::Active));
        let segmenting = ProgressUpdate {
            stage: Stage::Segmenting,
            ..ocr.clone()
        };
        assert_eq!(
            document_stage(StageInputs {
                parse_job: Some(&j),
                progress: Some(&segmenting),
                ..inputs(&f)
            })
            .stage,
            Stage::Segmenting
        );
        let t = jobs::claimed(job(TRANSCRIBE_JOB), "w", 200);
        assert_eq!(
            pair(&document_stage(StageInputs {
                parse_job: Some(&t),
                ..inputs(&f)
            })),
            (Stage::Transcription, StageStatus::Active)
        );
    }

    #[test]
    fn progress_from_another_job_or_a_stale_report_changes_nothing() {
        let f = facts();
        let j = jobs::claimed(job(RENDITION_JOB), "w", 200);
        let other = ProgressUpdate {
            job_id: id("other"),
            source_id: None,
            stage: Stage::Ocr,
            done: 1,
            total: None,
        };
        assert_eq!(
            document_stage(StageInputs {
                parse_job: Some(&j),
                progress: Some(&other),
                ..inputs(&f)
            })
            .stage,
            Stage::Parsing
        );
        let queued_again = jobs::failed(j, "boom", true, 300, 0);
        let mine = ProgressUpdate { job_id: id("j"), ..other };
        assert_eq!(
            document_stage(StageInputs {
                parse_job: Some(&queued_again),
                progress: Some(&mine),
                ..inputs(&f)
            })
            .stage,
            Stage::Parsing,
            "it isn't running, so a report from its last attempt doesn't count"
        );
    }

    #[test]
    fn retry_waiting_and_failed_paths_are_visible() {
        let f = facts();
        let held = jobs::claimed(job(RENDITION_JOB), "w", 200);
        let retrying = jobs::failed(held.clone(), "boom", true, 300, 0);
        let s = document_stage(StageInputs {
            parse_job: Some(&retrying),
            ..inputs(&f)
        });
        assert_eq!(pair(&s), (Stage::Parsing, StageStatus::Retrying));
        assert_eq!(s.attempts, 1);
        let waiting = jobs::waiting(held.clone(), "away", 300, 0);
        // Its attempt was taken back, and it's still waiting rather than merely queued.
        assert_eq!(
            document_stage(StageInputs {
                parse_job: Some(&waiting),
                ..inputs(&f)
            })
            .status,
            StageStatus::Waiting
        );
        let dead = jobs::failed(held.clone(), "bad", false, 300, 0);
        assert_eq!(
            pair(&document_stage(StageInputs {
                parse_job: Some(&dead),
                ..inputs(&f)
            })),
            (Stage::Parsing, StageStatus::Failed)
        );
        let cancelled = jobs::cancelled(held, 300).unwrap();
        assert_eq!(
            document_stage(StageInputs {
                parse_job: Some(&cancelled),
                ..inputs(&f)
            })
            .status,
            StageStatus::Failed
        );
    }

    #[test]
    fn a_failed_reprocess_shows_and_a_later_rendition_gets_past_an_old_failure() {
        let f = with_rendition();
        let dead_after = jobs::failed(jobs::claimed(job(RENDITION_JOB), "w", 2000), "bad", false, 3000, 0);
        assert_eq!(
            pair(&document_stage(StageInputs {
                parse_job: Some(&dead_after),
                ..inputs(&f)
            })),
            (Stage::Parsing, StageStatus::Failed)
        );
        let dead_before = jobs::failed(jobs::claimed(job(RENDITION_JOB), "w", 100), "bad", false, 500, 0);
        assert_eq!(
            document_stage(StageInputs {
                parse_job: Some(&dead_before),
                ..inputs(&f)
            })
            .stage,
            Stage::Done
        );
    }

    #[test]
    fn indexing_then_embedding_come_before_done() {
        let f = with_rendition();
        assert_eq!(
            pair(&document_stage(StageInputs {
                indexed_to_ms: Some(999),
                ..inputs(&f)
            })),
            (Stage::Indexing, StageStatus::Active)
        );
        for (state, status) in [
            (EmbedState::Queued, StageStatus::Queued),
            (EmbedState::Running, StageStatus::Active),
            (EmbedState::Dead, StageStatus::Failed),
        ] {
            assert_eq!(
                pair(&document_stage(StageInputs {
                    embed: state,
                    indexed_to_ms: Some(1000),
                    ..inputs(&f)
                })),
                (Stage::Embedding, status)
            );
        }
        assert_eq!(pair(&document_stage(inputs(&f))), (Stage::Done, StageStatus::Done));
    }

    #[test]
    fn extraction_states_map_to_the_stage() {
        let base = with_rendition();
        let doc = |row: ExtractionRow, j: JobRow| DocumentFacts {
            extraction: Some(row),
            extraction_job: Some(j),
            ..base.clone()
        };
        let queued = doc(extraction(None, 0, 0), job(EXTRACT_JOB));
        assert_eq!(pair(&document_stage(inputs(&queued))), (Stage::Extraction, StageStatus::Queued));
        let held = jobs::claimed(job(EXTRACT_JOB), "w", 1);
        let running = doc(extraction(None, 1, 4), held.clone());
        assert_eq!(pair(&document_stage(inputs(&running))), (Stage::Extraction, StageStatus::Active));
        let waiting = doc(extraction(None, 1, 4), jobs::waiting(held.clone(), "llm away", 5, 0));
        assert_eq!(pair(&document_stage(inputs(&waiting))), (Stage::Extraction, StageStatus::Waiting));
        let retrying = doc(extraction(None, 1, 4), jobs::failed(held.clone(), "refused", true, 5, 0));
        assert_eq!(pair(&document_stage(inputs(&retrying))), (Stage::Extraction, StageStatus::Retrying));
        let dead = doc(extraction(None, 0, 4), jobs::failed(held.clone(), "nothing usable", false, 5, 0));
        assert_eq!(pair(&document_stage(inputs(&dead))), (Stage::Extraction, StageStatus::Failed));
        let done = doc(extraction(None, 4, 4), jobs::succeeded(held.clone(), 5));
        assert_eq!(pair(&document_stage(inputs(&done))), (Stage::Done, StageStatus::Done));
    }

    #[test]
    fn partial_is_visible_at_the_end_and_while_review_waits() {
        let base = with_rendition();
        let held = jobs::claimed(job(EXTRACT_JOB), "w", 1);
        for row in [extraction(Some(ExtractionStop::Budget), 2, 4), extraction(None, 2, 4)] {
            let partial = DocumentFacts {
                extraction: Some(row),
                extraction_job: Some(jobs::succeeded(held.clone(), 5)),
                ..base.clone()
            };
            let s = document_stage(inputs(&partial));
            assert_eq!(pair(&s), (Stage::Done, StageStatus::Partial));
            assert!(s.partial);
            let review = DocumentFacts { pending_review: 3, ..partial };
            let s = document_stage(inputs(&review));
            assert_eq!(pair(&s), (Stage::Review, StageStatus::Active));
            assert!(s.partial, "the review stage doesn't hide that the run was partial");
        }
    }

    #[test]
    fn a_failed_page_or_a_skipped_stretch_makes_a_document_partial() {
        let mut r = rendition(1000);
        r.pages = vec![PageOutcome {
            index: 0,
            route: PageRoute::Ocr,
            mean_confidence_permille: None,
            flags: vec![PageFlag::OcrFailed { reason: "x".into() }],
        }];
        let f = DocumentFacts { rendition: Some(r), ..facts() };
        assert_eq!(pair(&document_stage(inputs(&f))), (Stage::Done, StageStatus::Partial));
        let mut r = rendition(1000);
        r.media = Some(MediaFacts {
            codec: "pcm".into(),
            sample_rate: 16_000,
            channels: 1,
            duration_ms: 1,
            gaps: 0,
            gap_ms: 0,
            packets: 1,
            bad_packets: 0,
            skipped_spans: 1,
            coarse_spans: 0,
        });
        let f = DocumentFacts { rendition: Some(r), ..facts() };
        assert_eq!(pair(&document_stage(inputs(&f))), (Stage::Done, StageStatus::Partial));
    }

    #[test]
    fn review_comes_after_an_extraction_is_done() {
        let f = DocumentFacts {
            extraction: Some(extraction(None, 4, 4)),
            extraction_job: Some(jobs::succeeded(jobs::claimed(job(EXTRACT_JOB), "w", 1), 5)),
            pending_review: 2,
            ..with_rendition()
        };
        assert_eq!(pair(&document_stage(inputs(&f))), (Stage::Review, StageStatus::Active));
    }

    #[test]
    fn an_earlier_failure_is_not_hidden_by_later_work() {
        let f = DocumentFacts {
            extraction: Some(extraction(None, 4, 4)),
            extraction_job: Some(jobs::succeeded(jobs::claimed(job(EXTRACT_JOB), "w", 1), 5)),
            pending_review: 2,
            ..with_rendition()
        };
        assert_eq!(
            pair(&document_stage(StageInputs {
                embed: EmbedState::Dead,
                ..inputs(&f)
            })),
            (Stage::Embedding, StageStatus::Failed)
        );
    }

    const LIMITS: BufferLimits = BufferLimits { max_frames: 4, max_bytes: 100 };

    #[test]
    fn durable_frames_overflow_only_at_the_limit() {
        assert_eq!(admit(FrameClass::Durable, 3, 10, 10, LIMITS), Admission::Accept);
        assert_eq!(admit(FrameClass::Durable, 4, 10, 10, LIMITS), Admission::Overflow);
        assert_eq!(admit(FrameClass::Durable, 1, 95, 10, LIMITS), Admission::Overflow);
        assert_eq!(
            admit(FrameClass::Durable, 0, 0, 1000, LIMITS),
            Admission::Accept,
            "an empty buffer takes a huge frame"
        );
    }

    #[test]
    fn progress_is_dropped_before_it_can_cause_an_overflow() {
        assert_eq!(admit(FrameClass::Progress, 1, 10, 10, LIMITS), Admission::Accept);
        assert_eq!(admit(FrameClass::Progress, 2, 10, 10, LIMITS), Admission::Drop);
        assert_eq!(admit(FrameClass::Progress, 0, 45, 10, LIMITS), Admission::Drop);
    }

    #[test]
    fn a_heartbeat_only_goes_to_an_empty_buffer() {
        assert_eq!(admit(FrameClass::Heartbeat, 0, 0, 10, LIMITS), Admission::Accept);
        assert_eq!(admit(FrameClass::Heartbeat, 1, 10, 10, LIMITS), Admission::Drop);
    }

    #[test]
    fn last_event_id_comes_first_and_nonsense_is_malformed() {
        assert_eq!(resume_request(None, None), ResumeRequest::Fresh);
        assert_eq!(resume_request(Some("7"), Some("3")), ResumeRequest::After(7));
        assert_eq!(resume_request(None, Some("3")), ResumeRequest::After(3));
        assert_eq!(resume_request(Some("-1"), None), ResumeRequest::After(-1));
        for bad in ["-2", "abc", "", "1e3", "99999999999999999999"] {
            assert_eq!(resume_request(Some(bad), None), ResumeRequest::Malformed, "{bad}");
        }
    }

    #[test]
    fn a_resume_goes_to_the_ring_the_log_or_a_resync() {
        use ResumePlan::*;
        let plan = |r, oldest| plan_resume(r, 100, oldest, 50);
        assert_eq!(plan(ResumeRequest::Fresh, None), Live { named: false });
        assert_eq!(plan(ResumeRequest::After(100), None), Live { named: true });
        assert_eq!(plan(ResumeRequest::After(101), None), Resync(ResyncReason::UnknownPosition));
        assert_eq!(plan(ResumeRequest::Malformed, Some(0)), Resync(ResyncReason::UnknownPosition));
        assert_eq!(plan(ResumeRequest::After(90), Some(80)), FromRing(90));
        assert_eq!(plan(ResumeRequest::After(79), Some(80)), FromRing(79), "the ring starts right after this");
        assert_eq!(plan(ResumeRequest::After(78), Some(80)), FromLog { after: 78, until: 100 });
        assert_eq!(
            plan(ResumeRequest::After(10), Some(0)),
            FromLog { after: 10, until: 100 },
            "too much for its buffer"
        );
        assert_eq!(plan(ResumeRequest::After(-1), None), FromLog { after: -1, until: 100 });
    }

    #[test]
    fn a_gap_is_found_where_it_is() {
        assert_eq!(first_gap(4, [5, 6, 7]), None);
        assert_eq!(first_gap(4, [6, 7]), Some(0));
        assert_eq!(first_gap(4, [5, 7]), Some(1));
        assert_eq!(first_gap(-1, [0, 1]), None);
        assert_eq!(first_gap(-1, [1]), Some(0));
        assert_eq!(first_gap(4, []), None);
    }

    fn update(job: &str, done: u64) -> ProgressUpdate {
        ProgressUpdate {
            job_id: id(job),
            source_id: None,
            stage: Stage::Ocr,
            done,
            total: Some(10),
        }
    }

    #[test]
    fn the_board_keeps_the_latest_per_job_and_forgets_finished_jobs() {
        let (b, v) = (ProgressBoard::default(), VaultId("v".into()));
        b.report(&v, update("a", 1), 1);
        b.report(&v, update("b", 1), 2);
        b.report(&v, update("a", 2), 3);
        let (seen, version) = b.since(&v, 0);
        assert_eq!(
            seen.iter().map(|e| (e.update.job_id.0.as_str(), e.update.done)).collect::<Vec<_>>(),
            [("b", 1), ("a", 2)]
        );
        assert_eq!(b.since(&v, version).0, vec![]);
        b.report(&v, update("a", 3), 4);
        assert_eq!(b.since(&v, version).0.len(), 1);
        b.finished(&v, &id("a"));
        assert_eq!(b.current(&v).len(), 1);
        assert_eq!(b.since(&VaultId("other".into()), 0).0, vec![]);
    }

    #[test]
    fn the_board_stays_bounded() {
        let (b, v) = (ProgressBoard::default(), VaultId("v".into()));
        for n in 0..(BOARD_ENTRIES as u64 + 50) {
            b.report(&v, update(&format!("j{n}"), n), 0);
        }
        let current = b.current(&v);
        assert_eq!(current.len(), BOARD_ENTRIES);
        assert!(!current.iter().any(|e| e.update.job_id == id("j0")), "the oldest report went");
    }

    #[test]
    fn every_kind_of_document_job_has_a_stage_and_others_have_none() {
        assert_eq!(stage_of_job(RENDITION_JOB), Some(Stage::Parsing));
        assert_eq!(stage_of_job(TRANSCRIBE_JOB), Some(Stage::Transcription));
        assert_eq!(stage_of_job(EMBED_JOB), Some(Stage::Embedding));
        assert_eq!(stage_of_job(EXTRACT_JOB), Some(Stage::Extraction));
        assert_eq!(stage_of_job(REVIEW_JOB), Some(Stage::Review));
        assert_eq!(stage_of_job(crate::AUDIT_JOB), None);
        assert_eq!(stage_of_job("anything else"), None);
    }

    #[test]
    fn every_stage_has_a_name_that_parses_back() {
        for s in Stage::ALL {
            assert_eq!(Stage::parse(s.as_str()), Some(s));
        }
        assert_eq!(Stage::parse("nope"), None);
    }
}
