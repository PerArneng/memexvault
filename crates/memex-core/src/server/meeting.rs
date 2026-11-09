//! Recordings on the facade: their meeting metadata, transcript, media facts and bytes.
use super::{Proposal, ProposalOutcome, Server, ServerEdges, ServerError};
use crate::auth::Caller;
use crate::meeting::{self, TimedIndex, TranscriptPart};
use crate::{BlobStore, Snapshot, Store};
use memex_types::{Anchor, MediaFacts, MeetingMeta, ObjectId, Op, Participant, RenditionRow, Row, RowKey, SourceRow, Table, TimedDocument, VaultId};
use std::io::Read;

/// A recording's facts. `media` is empty until its transcription finishes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaInfo {
    pub source: SourceRow,
    pub rendition_id: Option<ObjectId>,
    pub media: Option<MediaFacts>,
}

/// A participant, with the Person a committed proposal linked them to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParticipantView {
    pub participant: Participant,
    pub entity_id: Option<ObjectId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MeetingView {
    pub source: SourceRow,
    pub title: Option<String>,
    pub date_ms: Option<i64>,
    pub participants: Vec<ParticipantView>,
}

/// Which part of a transcript to read: words from `after` on, overlapping these times.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TranscriptWindow {
    pub from_ms: i64,
    pub to_ms: i64,
    pub after: usize,
    pub limit: usize,
}

/// A proposal that a participant is this Person.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParticipantLink {
    pub participant: String,
    pub entity: ObjectId,
    pub rationale: String,
    pub idempotency_key: String,
}

/// A page of a transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptPage {
    pub rendition_id: ObjectId,
    pub parser: String,
    pub duration_ms: i64,
    pub parts: Vec<TranscriptPart>,
    /// Each part's words, on the table's own times, with no padding.
    pub words: Vec<Vec<memex_types::TimedWord>>,
    /// The word to carry on from.
    pub next: Option<usize>,
}

impl<E: ServerEdges> Server<E> {
    /// A rendition's word table when it's a recording's. Reading it fails rather than returns nothing, as a recording's anchor without its times would be broken.
    pub(super) fn timed_of(&self, vault: &VaultId, r: &RenditionRow) -> Result<Option<TimedIndex>, ServerError> {
        if r.media.is_none() {
            return Ok(None);
        }
        let table = self.word_table_bytes(vault, &r.words_blob)?;
        let doc: TimedDocument = serde_json::from_slice(&table).map_err(|e| ServerError::Unavailable(format!("a word table can't be read: {e}")))?;
        Ok(Some(TimedIndex::new(doc)))
    }

    /// An anchor with its time, when its rendition is a recording's. None means it should have one and can't, which is a broken anchor.
    pub(super) fn with_time(mut anchor: Anchor, timed: Option<&TimedIndex>) -> Option<Anchor> {
        if let Some(index) = timed {
            anchor.time = Some(index.span_of(anchor.start, anchor.end)?);
        }
        Some(anchor)
    }

    fn recording(&self, caller: &Caller, vault: &VaultId, id: &ObjectId) -> Result<SourceRow, ServerError> {
        let source = self.get_source(caller, vault, id)?;
        if !meeting::is_media(&source.media_type) {
            return Err(ServerError::Invalid(format!("this source is {}, not a recording", source.media_type)));
        }
        Ok(source)
    }

    fn current_of(&self, vault: &VaultId, source: &ObjectId) -> Result<Option<RenditionRow>, ServerError> {
        Ok(self
            .engine
            .store()
            .snapshot(vault)?
            .renditions_of(source)?
            .into_iter()
            .rfind(|r| r.superseded_by.is_none()))
    }

    /// A recording's media type and, once transcribed, its duration and what decoding found.
    pub fn media_info(&self, caller: &Caller, vault: &VaultId, id: &ObjectId) -> Result<MediaInfo, ServerError> {
        let source = self.recording(caller, vault, id)?;
        let current = self.current_of(vault, id)?;
        Ok(MediaInfo {
            source,
            rendition_id: current.as_ref().map(|r| r.header.id.clone()),
            media: current.and_then(|r| r.media),
        })
    }

    /// A recording's title, date and participants. With no title set, the title is the file's name.
    pub fn meeting(&self, caller: &Caller, vault: &VaultId, id: &ObjectId) -> Result<MeetingView, ServerError> {
        let source = self.recording(caller, vault, id)?;
        let meta = meeting::meeting_of(&source.metadata);
        let keys: Vec<RowKey> = meta
            .participants
            .iter()
            .map(|p| RowKey {
                table: Table::Edge,
                id: meeting::participant_edge_id(&source.logical_id.0, &p.id),
            })
            .collect();
        let edges = self.engine.store().snapshot(vault)?.get_many(&keys)?;
        let participants = meta
            .participants
            .into_iter()
            .zip(edges)
            .map(|(participant, edge)| ParticipantView {
                participant,
                entity_id: match edge {
                    Some(Row::Edge(e)) if e.kind == meeting::PARTICIPANT_EDGE => Some(e.to),
                    _ => None,
                },
            })
            .collect();
        Ok(MeetingView {
            title: meta.title.or_else(|| Some(source.name.clone())),
            date_ms: meta.date_ms,
            participants,
            source,
        })
    }

    /// Proposes new meeting metadata. It's a proposal like any other edit to a Source: a person's commits at once, and an agent's waits for review. A participant with no id gets one here, so the links can follow them.
    pub fn propose_meeting(
        &self,
        caller: &Caller,
        vault: &VaultId,
        id: &ObjectId,
        mut meta: MeetingMeta,
        base_version: u32,
        idempotency_key: String,
    ) -> Result<ProposalOutcome, ServerError> {
        let source = self.recording(caller, vault, id)?;
        for p in &mut meta.participants {
            if p.id.is_empty() {
                p.id = self.new_id();
            }
        }
        meeting::check_meeting(&meta).map_err(ServerError::Invalid)?;
        let ops = vec![Op::SetSourceMetadata {
            id: id.clone(),
            base_version,
            metadata: meeting::with_meeting(&source.metadata, &meta),
        }];
        self.propose(
            caller,
            vault,
            Proposal {
                ops,
                rationale: "meeting metadata".into(),
                idempotency_key,
            },
            false,
        )
    }

    /// Proposes that a participant is this Person, as an Edge from the recording to the entity. The read shows it once it commits, and reverting the ChangeSet takes it back.
    pub fn propose_participant_link(&self, caller: &Caller, vault: &VaultId, id: &ObjectId, link: ParticipantLink) -> Result<ProposalOutcome, ServerError> {
        let ParticipantLink {
            participant,
            entity,
            rationale,
            idempotency_key,
        } = link;
        let source = self.recording(caller, vault, id)?;
        if !meeting::meeting_of(&source.metadata).participants.iter().any(|p| p.id == participant) {
            return Err(ServerError::NotFound);
        }
        let row = self.engine.store().snapshot(vault)?.get(&RowKey {
            table: Table::Entity,
            id: entity.clone(),
        })?;
        match row {
            Some(Row::Entity(e)) if e.type_name == "Person" => {}
            Some(Row::Entity(e)) => return Err(ServerError::Invalid(format!("a participant links to a Person, not a {}", e.type_name))),
            _ => return Err(ServerError::Invalid(format!("there's no entity {entity}"))),
        }
        let ops = vec![Op::AddEdge {
            id: meeting::participant_edge_id(&source.logical_id.0, &participant),
            from: source.logical_id.clone(),
            to: entity,
            kind: meeting::PARTICIPANT_EDGE.into(),
        }];
        self.propose(
            caller,
            vault,
            Proposal {
                ops,
                rationale,
                idempotency_key,
            },
            false,
        )
    }

    /// The transcript between two times, from word `after` on. With no window it's all of it, a page at a time.
    pub fn transcript(&self, caller: &Caller, vault: &VaultId, id: &ObjectId, window: TranscriptWindow) -> Result<TranscriptPage, ServerError> {
        self.recording(caller, vault, id)?;
        let Some(current) = self.current_of(vault, id)? else {
            return Err(ServerError::Conflict("this recording hasn't been transcribed yet".into()));
        };
        let Some(index) = self.timed_of(vault, &current)? else {
            return Err(ServerError::Conflict("this recording has no timed transcript".into()));
        };
        let (parts, next) = meeting::transcript_parts(&index, window.from_ms, window.to_ms, window.after, window.limit);
        let words = parts.iter().map(|p| index.doc.words[p.words.clone()].to_vec()).collect();
        Ok(TranscriptPage {
            rendition_id: current.header.id,
            parser: current.parser,
            duration_ms: index.doc.media.duration_ms,
            parts,
            words,
            next,
        })
    }

    /// A recording to copy for transcription. It's written out in pieces, as it can be larger than memory should hold.
    pub fn copy_source(&self, vault: &VaultId, id: &ObjectId, out: &mut dyn std::io::Write) -> Result<SourceRow, ServerError> {
        let source = self.source_row(vault, id)?;
        let hash = memex_types::BlobHash::parse(&source.sha256).map_err(|e| ServerError::Invalid(e.to_string()))?;
        let mut reader = self.blobs.read(vault, &hash, 0, None)?;
        std::io::copy(&mut reader, out).map_err(|e| ServerError::Unavailable(e.to_string()))?;
        Ok(source)
    }

    /// Says whether a recording's file is there, for a route that has to choose its status before it reads anything.
    pub fn check_recording_blob(&self, caller: &Caller, vault: &VaultId, id: &ObjectId) -> Result<(), ServerError> {
        let source = self.recording(caller, vault, id)?;
        let hash = memex_types::BlobHash::parse(&source.sha256).map_err(|e| ServerError::Invalid(e.to_string()))?;
        self.blobs.read(vault, &hash, 0, Some(0))?;
        Ok(())
    }

    /// A recording's bytes from `offset`, at most `len`, handed to `sink` in pieces until it says stop. Memory stays flat however large the file or the range.
    pub fn stream_recording(
        &self,
        caller: &Caller,
        vault: &VaultId,
        id: &ObjectId,
        offset: u64,
        len: u64,
        sink: &mut dyn FnMut(&[u8]) -> bool,
    ) -> Result<(), ServerError> {
        let source = self.recording(caller, vault, id)?;
        let hash = memex_types::BlobHash::parse(&source.sha256).map_err(|e| ServerError::Invalid(e.to_string()))?;
        let mut reader = self.blobs.read(vault, &hash, offset, Some(len))?;
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            let n = reader.read(&mut buffer).map_err(|e| ServerError::Unavailable(e.to_string()))?;
            if n == 0 || !sink(&buffer[..n]) {
                return Ok(());
            }
        }
    }

    /// The language hint a recording was uploaded with, for its job.
    pub fn source_language(&self, vault: &VaultId, id: &ObjectId) -> Result<Option<String>, ServerError> {
        Ok(meeting::language_of(&self.source_row(vault, id)?.metadata))
    }

    /// A job's state, or None when there's no such job.
    pub fn job_state(&self, vault: &VaultId, id: &ObjectId) -> Result<Option<memex_types::JobState>, ServerError> {
        match self.job(vault, id) {
            Ok(j) => Ok(Some(j.state)),
            Err(ServerError::NotFound) => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// A recording's row, for a read that has to answer before the bytes start.
    pub fn recording_row(&self, caller: &Caller, vault: &VaultId, id: &ObjectId) -> Result<SourceRow, ServerError> {
        self.recording(caller, vault, id)
    }
}
