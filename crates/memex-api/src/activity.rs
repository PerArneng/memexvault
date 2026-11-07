//! What the activity stream sends. A frame is built field by field from a fixed list: ids, kinds, stages, counts and timings. Nothing is copied across wholesale, so an event kind or row added later carries nothing into a frame until someone lists a field here. The row match has no catch-all, so a new row kind has to be decided on, and the decision that needs no thought is to count it.
use memex_core::{ActivitySnapshot, ChangeSetRecorded, DocumentActivity, ResyncReason, StateRecorded, upcast};
use memex_types::{
    AuditRunRow, AuditRunState, ChangeSetRow, ChangeSetState, DerivationOutcome, DerivationRow, Event, ExtractionRow, JobRow, JobState, Mutation, ObjectId,
    ProgressEntry, RenditionRow, Row, SourceRow, Table,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use utoipa::{IntoParams, ToSchema};

/// The most items of each list in a frame, so one large commit can't make a large frame. `omitted` says how many didn't fit.
pub const FRAME_LIST_LIMIT: usize = 32;
/// The longest id a frame carries.
const MAX_ID_LEN: usize = 64;

/// An id, if it looks like one. The server makes the ids that reach here, and this is what stops anything else being taken for one, such as a link's `slug:` target.
fn safe_id(id: &str) -> Option<String> {
    let fits = !id.is_empty() && id.len() <= MAX_ID_LEN && id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    fits.then(|| id.to_string())
}

fn id_of(id: &ObjectId) -> Option<String> {
    safe_id(&id.0)
}

/// Only the kinds named here keep their name, and the rest are `other`.
fn kind_name(kind: &str) -> &'static str {
    match kind {
        "state.recorded" => "state.recorded",
        "changeset.recorded" => "changeset.recorded",
        "segment.recorded" => "segment.recorded",
        "source.recorded" => "source.recorded",
        "page.created" => "page.created",
        "page.edited" => "page.edited",
        "page.deleted" => "page.deleted",
        "edge.linked" => "edge.linked",
        "edge.unlinked" => "edge.unlinked",
        _ => "other",
    }
}

fn job_kind_name(kind: &str) -> &'static str {
    match kind {
        memex_core::RENDITION_JOB => memex_core::RENDITION_JOB,
        memex_core::TRANSCRIBE_JOB => memex_core::TRANSCRIBE_JOB,
        memex_core::EMBED_JOB => memex_core::EMBED_JOB,
        memex_core::EXTRACT_JOB => memex_core::EXTRACT_JOB,
        memex_core::REVIEW_JOB => memex_core::REVIEW_JOB,
        memex_core::AUDIT_JOB => memex_core::AUDIT_JOB,
        _ => "other",
    }
}

fn table_name(t: Table) -> &'static str {
    match t {
        Table::Source => "source",
        Table::Page => "page",
        Table::Edge => "edge",
        Table::Segment => "segment",
        Table::Assertion => "assertion",
        Table::Evidence => "evidence",
        Table::ChangeSet => "changeset",
        Table::Job => "job",
        Table::Rendition => "rendition",
        Table::EntityType => "entity_type",
        Table::Entity => "entity",
        Table::Block => "block",
        Table::Slug => "slug",
        Table::Revision => "revision",
        Table::Derivation => "derivation",
        Table::Extraction => "extraction",
        Table::ReviewVerdict => "review_verdict",
        Table::AutoCommit => "auto_commit",
        Table::AuditRun => "audit_run",
        Table::AuditCheck => "audit_check",
        Table::AuditState => "audit_state",
        Table::AuditPlanted => "audit_planted",
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct SourceFact {
    pub id: String,
    pub logical_id: Option<String>,
    pub version_no: u32,
    pub size: u64,
}

impl SourceFact {
    fn of(s: &SourceRow) -> Option<Self> {
        Some(Self {
            id: id_of(&s.header.id)?,
            logical_id: id_of(&s.logical_id),
            version_no: s.version_no,
            size: s.size,
        })
    }
}

/// A job at one moment. Its error messages, its input and who holds it stay out, as an error can quote a document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct JobFact {
    pub id: String,
    /// `rendition`, `transcription`, `embed`, `extract`, `review`, `audit` or `other`.
    pub kind: String,
    /// `queued`, `running`, `done`, `dead` or `cancelled`.
    pub state: String,
    /// The stage this job's work belongs to, when it's about a document.
    pub stage: Option<String>,
    /// `queued`, `active`, `waiting`, `retrying`, `failed` or `done`.
    pub status: String,
    pub attempts: u32,
    pub max_attempts: u32,
    pub next_run_at_ms: i64,
    pub finished_at_ms: Option<i64>,
    pub waiting_since_ms: Option<i64>,
    pub source_id: Option<String>,
    pub rendition_id: Option<String>,
    pub changeset_id: Option<String>,
    pub audit_run_id: Option<String>,
    /// How many objects an embedding job holds.
    pub docs: Option<u32>,
}

fn input_id(input: &Value, name: &str) -> Option<String> {
    input.get(name).and_then(Value::as_str).and_then(safe_id)
}

fn job_state_name(state: JobState) -> &'static str {
    match state {
        JobState::Queued => "queued",
        JobState::Running => "running",
        JobState::Done => "done",
        JobState::Dead => "dead",
        JobState::Cancelled => "cancelled",
    }
}

impl JobFact {
    fn of(j: &JobRow) -> Option<Self> {
        Some(Self {
            id: id_of(&j.header.id)?,
            kind: job_kind_name(&j.kind).into(),
            state: job_state_name(j.state).into(),
            stage: memex_core::activity::stage_of_job(&j.kind).map(|s| s.as_str().into()),
            status: memex_core::activity::status_of_job(j).as_str().into(),
            attempts: j.attempts.len() as u32,
            max_attempts: j.max_attempts,
            next_run_at_ms: j.next_run_at_ms,
            finished_at_ms: j.finished_at_ms,
            waiting_since_ms: j.wait.as_ref().map(|w| w.since_ms),
            source_id: input_id(&j.input, "source_id"),
            rendition_id: input_id(&j.input, "rendition_id"),
            changeset_id: input_id(&j.input, "changeset_id"),
            audit_run_id: input_id(&j.input, "run_id"),
            docs: j.input.get("docs").and_then(Value::as_array).map(|d| d.len() as u32),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct RenditionFact {
    pub id: String,
    pub source_id: Option<String>,
    pub page_count: u32,
    pub segment_count: u32,
    pub created_at_ms: i64,
    pub superseded: bool,
}

impl RenditionFact {
    fn of(r: &RenditionRow) -> Option<Self> {
        Some(Self {
            id: id_of(&r.header.id)?,
            source_id: id_of(&r.source_id),
            page_count: r.page_count,
            segment_count: r.segment_count,
            created_at_ms: r.created_at_ms,
            superseded: r.superseded_by.is_some(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ExtractionFact {
    pub rendition_id: String,
    pub source_id: Option<String>,
    pub job_id: Option<String>,
    pub segments_total: u32,
    pub segments_done: u32,
    pub batches_failed: u32,
    pub proposed: u32,
    pub rejected: u32,
    pub tokens: u64,
    pub token_budget: u64,
    /// `budget`, `failed_batches` or `superseded`, when a run ended short.
    pub stop: Option<String>,
}

impl ExtractionFact {
    fn of(x: &ExtractionRow) -> Option<Self> {
        Some(Self {
            rendition_id: id_of(&x.header.id)?,
            source_id: id_of(&x.source_id),
            job_id: id_of(&x.job_id),
            segments_total: x.segments_total,
            segments_done: x.segments_done,
            batches_failed: x.batches_failed,
            proposed: x.proposed,
            rejected: x.rejected,
            tokens: x.usage.total(),
            token_budget: x.token_budget,
            stop: x.stop.map(|s| memex_core::extraction::stop_text(s).into()),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct DerivationFact {
    pub id: String,
    pub job_id: Option<String>,
    pub source_id: Option<String>,
    pub rendition_id: Option<String>,
    /// `done` or `failed`.
    pub outcome: String,
    pub segments: u32,
    pub items: u32,
    pub refused: u32,
    pub tokens: u64,
}

impl DerivationFact {
    fn of(d: &DerivationRow) -> Option<Self> {
        Some(Self {
            id: id_of(&d.header.id)?,
            job_id: id_of(&d.job_id),
            source_id: id_of(&d.source_id),
            rendition_id: id_of(&d.rendition_id),
            outcome: match d.outcome {
                DerivationOutcome::Done => "done",
                DerivationOutcome::Failed => "failed",
            }
            .into(),
            segments: d.segment_ids.len() as u32,
            items: d.items.len() as u32,
            refused: d.rejected.len() as u32,
            tokens: d.usage.total(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ChangeSetFact {
    pub id: String,
    /// `pending`, `committed` or `rejected`.
    pub state: String,
    pub machine: bool,
    pub ops: u32,
    pub proposed_at_ms: i64,
    pub decided_at_ms: Option<i64>,
    pub commit_seq: Option<u64>,
}

fn changeset_state_name(state: ChangeSetState) -> &'static str {
    match state {
        ChangeSetState::Pending => "pending",
        ChangeSetState::Committed => "committed",
        ChangeSetState::Rejected => "rejected",
    }
}

impl ChangeSetFact {
    fn of(c: &ChangeSetRow) -> Option<Self> {
        Some(Self {
            id: id_of(&c.header.id)?,
            state: changeset_state_name(c.state).into(),
            machine: c.proposer_is_machine,
            ops: c.ops.len() as u32,
            proposed_at_ms: c.proposed_at_ms,
            decided_at_ms: c.decided_at_ms,
            commit_seq: c.commit_seq,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct AuditRunFact {
    pub id: String,
    pub job_id: Option<String>,
    /// `queued`, `running` or `finished`.
    pub state: String,
    pub sample_size: u32,
    pub sampled: u32,
    pub passed: u32,
    pub suspect: u32,
    pub failed: u32,
    pub llm_calls: u32,
    pub finished_at_ms: Option<i64>,
    /// Why its LLM checks stopped short, in a word.
    pub stop: Option<String>,
}

impl AuditRunFact {
    fn of(r: &AuditRunRow) -> Option<Self> {
        Some(Self {
            id: id_of(&r.header.id)?,
            job_id: id_of(&r.job_id),
            state: match r.state {
                AuditRunState::Queued => "queued",
                AuditRunState::Running => "running",
                AuditRunState::Finished => "finished",
            }
            .into(),
            sample_size: r.sample_size,
            sampled: r.sampled,
            passed: r.passed,
            suspect: r.suspect,
            failed: r.failed,
            llm_calls: r.llm_calls,
            finished_at_ms: r.finished_at_ms,
            stop: r.stop.map(|s| s.as_str().into()),
        })
    }
}

/// One event from the log, reduced to what can safely be said about it. Its `seq` is the frame's SSE id.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct DurableFrame {
    pub seq: u64,
    pub time_ms: i64,
    /// The event's kind, or `other` for one this server doesn't list.
    pub kind: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sources: Vec<SourceFact>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub jobs: Vec<JobFact>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub renditions: Vec<RenditionFact>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub extractions: Vec<ExtractionFact>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub derivations: Vec<DerivationFact>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changesets: Vec<ChangeSetFact>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub audit_runs: Vec<AuditRunFact>,
    /// How many rows of each kind the event wrote.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub counts: BTreeMap<String, u32>,
    /// How many items didn't fit in the lists above.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub omitted: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

fn push<T>(list: &mut Vec<T>, omitted: &mut u32, item: Option<T>) {
    match item {
        Some(i) if list.len() < FRAME_LIST_LIMIT => list.push(i),
        _ => *omitted += 1,
    }
}

impl DurableFrame {
    fn count(&mut self, table: Table) {
        *self.counts.entry(table_name(table).into()).or_insert(0) += 1;
    }

    fn add_mutations(&mut self, mutations: &[Mutation]) {
        for m in mutations {
            match m {
                Mutation::Delete(key) => self.count(key.table),
                Mutation::Put(row) => {
                    self.count(row.key().table);
                    self.add_row(row);
                }
            }
        }
    }

    /// No catch-all: a new kind of row won't compile until it's listed here. Only what's built by a `Fact` above comes through.
    fn add_row(&mut self, row: &Row) {
        match row {
            Row::Source(x) => push(&mut self.sources, &mut self.omitted, SourceFact::of(x)),
            Row::Job(x) => push(&mut self.jobs, &mut self.omitted, JobFact::of(x)),
            Row::Rendition(x) => push(&mut self.renditions, &mut self.omitted, RenditionFact::of(x)),
            Row::Extraction(x) => push(&mut self.extractions, &mut self.omitted, ExtractionFact::of(x)),
            Row::Derivation(x) => push(&mut self.derivations, &mut self.omitted, DerivationFact::of(x)),
            Row::ChangeSet(x) => push(&mut self.changesets, &mut self.omitted, ChangeSetFact::of(x)),
            Row::AuditRun(x) => push(&mut self.audit_runs, &mut self.omitted, AuditRunFact::of(x)),
            Row::Page(_)
            | Row::Edge(_)
            | Row::Segment(_)
            | Row::Assertion(_)
            | Row::Evidence(_)
            | Row::EntityType(_)
            | Row::Entity(_)
            | Row::Block(_)
            | Row::Slug(_)
            | Row::Revision(_)
            | Row::ReviewVerdict(_)
            | Row::AutoCommit(_)
            | Row::AuditCheck(_)
            | Row::AuditState(_)
            | Row::AuditPlanted(_) => {}
        }
    }
}

/// The only fields read from a `source.recorded` event.
#[derive(Deserialize)]
struct SourceRecordedFacts {
    id: String,
    #[serde(default)]
    logical_id: Option<String>,
    #[serde(default)]
    version_no: u32,
    #[serde(default)]
    size: u64,
}

/// What may be said about one event. A payload that can't be read, or that erasure has emptied, leaves a frame with no details, as the envelope alone is enough to keep positions in order.
pub fn frame_of(event: &Event) -> DurableFrame {
    let mut frame = DurableFrame {
        seq: event.seq,
        time_ms: event.time_ms,
        kind: kind_name(&event.kind).into(),
        ..DurableFrame::default()
    };
    let Ok(current) = upcast(event) else { return frame };
    match frame.kind.as_str() {
        "state.recorded" => {
            if let Ok(p) = serde_json::from_value::<StateRecorded>(current.payload) {
                frame.add_mutations(&p.mutations);
            }
        }
        "changeset.recorded" => {
            if let Ok(p) = serde_json::from_value::<ChangeSetRecorded>(current.payload) {
                frame.count(Table::ChangeSet);
                push(&mut frame.changesets, &mut frame.omitted, ChangeSetFact::of(&p.changeset));
                frame.add_mutations(&p.mutations);
            }
        }
        "source.recorded" => {
            if let Ok(p) = serde_json::from_value::<SourceRecordedFacts>(current.payload) {
                let fact = safe_id(&p.id).map(|id| SourceFact {
                    id,
                    logical_id: p.logical_id.as_deref().and_then(safe_id),
                    version_no: p.version_no,
                    size: p.size,
                });
                push(&mut frame.sources, &mut frame.omitted, fact);
            }
        }
        // Their kind says what happened. What they're about can be asked for by anyone allowed to read it.
        _ => {}
    }
    frame
}

/// What a worker last said about a job. In memory only, so it has no id and no resume.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ProgressFrame {
    pub job_id: String,
    pub source_id: Option<String>,
    pub stage: String,
    pub done: u64,
    pub total: Option<u64>,
    pub time_ms: i64,
}

impl ProgressFrame {
    pub fn of(e: &ProgressEntry) -> Option<Self> {
        Some(Self {
            job_id: id_of(&e.update.job_id)?,
            source_id: e.update.source_id.as_ref().and_then(id_of),
            stage: e.update.stage.as_str().into(),
            done: e.update.done,
            total: e.update.total,
            time_ms: e.time_ms,
        })
    }
}

/// Fetch a new snapshot, then carry on from here. Also the frame's SSE id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ResyncFrame {
    /// The position this stream continues from. -1 is before the first event.
    pub head: i64,
    /// `overflow`, `unknown_position`, `log_gap` or `erased`.
    pub reason: String,
}

impl ResyncFrame {
    pub fn new(head: i64, reason: ResyncReason) -> Self {
        Self {
            head,
            reason: reason.as_str().into(),
        }
    }
}

/// The first frame of every stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct HelloFrame {
    /// Where the log's reader was as this stream opened. -1 is before the first event.
    pub head: i64,
    /// Whether this stream carries on from a position the client named.
    pub resumed: bool,
}

/// Says the stream is alive when nothing else has.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct HeartbeatFrame {
    pub head: i64,
}

/// The last frame of a stream, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct EndFrame {
    /// `auth_ended` (the token or session is over), `scope_lost` (it can't read this vault now), `vault_gone`, `unverifiable` (the store was away too long to check), `unavailable` or `server_stopping`.
    pub reason: String,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct ActivityParams {
    /// A position to carry on from, such as a snapshot's `head`, for clients that can't set `Last-Event-ID`. That header comes first when both are there.
    pub after: Option<String>,
}

#[derive(Debug, Deserialize, IntoParams)]
pub struct ActivityDocumentParams {
    pub cursor: Option<String>,
    /// At most 200. Defaults to 50.
    pub limit: Option<usize>,
    /// Only documents in this stage.
    pub stage: Option<String>,
}

/// How far a worker has got.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct DocumentProgressDto {
    pub stage: String,
    pub done: u64,
    pub total: Option<u64>,
    pub time_ms: i64,
}

/// One document on the live board.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct DocumentActivityDto {
    pub source_id: String,
    pub logical_id: String,
    pub version_no: u32,
    pub recorded_at_ms: i64,
    /// `stored`, `parsing`, `ocr`, `transcription`, `segmenting`, `indexing`, `embedding`, `extraction`, `review` or `done`.
    pub stage: String,
    /// `queued`, `active`, `waiting`, `retrying`, `failed`, `partial` or `done`.
    pub status: String,
    /// Some part of it didn't work, whatever stage it's in now.
    pub partial: bool,
    /// The job this depends on now, if one does.
    pub job_id: Option<String>,
    pub attempts: u32,
    pub waiting_since_ms: Option<i64>,
    pub next_run_at_ms: Option<i64>,
    pub rendition_id: Option<String>,
    pub page_count: Option<u32>,
    pub segment_count: Option<u32>,
    pub segments_extracted: Option<u32>,
    pub pending_review: u64,
    pub progress: Option<DocumentProgressDto>,
}

impl From<DocumentActivity> for DocumentActivityDto {
    fn from(d: DocumentActivity) -> Self {
        Self {
            source_id: d.source.header.id.0,
            logical_id: d.source.logical_id.0,
            version_no: d.source.version_no,
            recorded_at_ms: d.source.recorded_at_ms,
            stage: d.stage.stage.as_str().into(),
            status: d.stage.status.as_str().into(),
            partial: d.stage.partial,
            job_id: d.stage.job_id.map(|j| j.0),
            attempts: d.stage.attempts,
            waiting_since_ms: d.waiting_since_ms,
            next_run_at_ms: d.next_run_at_ms,
            rendition_id: d.facts.rendition.as_ref().map(|r| r.header.id.0.clone()),
            page_count: d.facts.rendition.as_ref().map(|r| r.page_count),
            segment_count: d.facts.rendition.as_ref().map(|r| r.segment_count),
            segments_extracted: d.facts.extraction.as_ref().map(|x| x.segments_done),
            pending_review: d.facts.pending_review,
            progress: d.progress.map(|e| DocumentProgressDto {
                stage: e.update.stage.as_str().into(),
                done: e.update.done,
                total: e.update.total,
                time_ms: e.time_ms,
            }),
        }
    }
}

/// A page of the live board.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
pub struct ActivityDocumentPage {
    /// The log position this reflects, which a stream can follow from with `after`. -1 is before the first event.
    pub head: i64,
    pub items: Vec<DocumentActivityDto>,
    pub next_cursor: Option<String>,
}

/// A page from a snapshot that was asked for one document more than it wants, so its cursor is exact.
pub fn activity_page(snapshot: ActivitySnapshot, limit: usize) -> ActivityDocumentPage {
    let head = snapshot.head.map_or(-1, |h| h as i64);
    let scan_cursor = snapshot.next_cursor.map(|c| c.0);
    let (items, full_cursor) = crate::page_of(snapshot.documents, limit, |d| d.source.header.id.0.clone());
    ActivityDocumentPage {
        head,
        items: items.into_iter().map(Into::into).collect(),
        next_cursor: full_cursor.or(scan_cursor),
    }
}
