//! Request and response types for recordings: meeting metadata, the transcript, and media facts.
use crate::ProposalResult;
use memex_core::{MediaInfo, MeetingView, TranscriptPage};
use memex_types::{MeetingMeta, Participant};
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ParticipantDto {
    /// Stable, so a link can follow them. The server gives one to a participant sent without it.
    pub id: String,
    pub name: String,
    /// A transcript's label for them, when someone knows it.
    pub speaker: Option<String>,
    /// The Person a committed proposal links them to.
    pub entity_id: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct MeetingDto {
    pub source_id: String,
    /// The version to base an edit on.
    pub version: u32,
    /// With no title set, the file's name.
    pub title: Option<String>,
    pub date_ms: Option<i64>,
    pub participants: Vec<ParticipantDto>,
}

impl From<MeetingView> for MeetingDto {
    fn from(m: MeetingView) -> Self {
        Self {
            source_id: m.source.header.id.0,
            version: m.source.header.version,
            title: m.title,
            date_ms: m.date_ms,
            participants: m
                .participants
                .into_iter()
                .map(|p| ParticipantDto {
                    id: p.participant.id,
                    name: p.participant.name,
                    speaker: p.participant.speaker,
                    entity_id: p.entity_id.map(|e| e.0),
                })
                .collect(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ParticipantInput {
    /// Leave out for a new participant.
    pub id: Option<String>,
    pub name: String,
    pub speaker: Option<String>,
}

/// A full replacement of a meeting's metadata, as a proposal.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct MeetingBody {
    pub title: Option<String>,
    pub date_ms: Option<i64>,
    pub participants: Vec<ParticipantInput>,
    pub base_version: u32,
    pub idempotency_key: String,
}

impl MeetingBody {
    pub fn meta(self) -> (MeetingMeta, u32, String) {
        let meta = MeetingMeta {
            title: self.title,
            date_ms: self.date_ms,
            participants: self
                .participants
                .into_iter()
                .map(|p| Participant {
                    id: p.id.unwrap_or_default(),
                    name: p.name,
                    speaker: p.speaker,
                })
                .collect(),
        };
        (meta, self.base_version, self.idempotency_key)
    }
}

/// Proposes that a participant is this Person.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ParticipantLinkBody {
    pub entity_id: String,
    pub rationale: String,
    pub idempotency_key: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct MeetingProposalResult {
    pub source_id: String,
    pub result: ProposalResult,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct MediaInfoDto {
    pub source_id: String,
    pub media_type: String,
    pub size: u64,
    /// The current rendition, once transcribed.
    pub rendition_id: Option<String>,
    /// Empty until transcribed, as decoding is what finds it.
    pub duration_ms: Option<i64>,
    pub codec: Option<String>,
    pub sample_rate: Option<u32>,
    pub channels: Option<u32>,
    /// Holes in the audio, kept as silence so times stay on the original timeline.
    pub gaps: Option<u32>,
    pub gap_ms: Option<i64>,
    /// Stretches the transcriber crashed on twice, which have no words.
    pub skipped_spans: Option<u32>,
    /// Stretches read with less accurate times, because the transcriber crashed on them once.
    pub coarse_spans: Option<u32>,
}

impl From<MediaInfo> for MediaInfoDto {
    fn from(i: MediaInfo) -> Self {
        let m = i.media.as_ref();
        Self {
            source_id: i.source.header.id.0,
            media_type: i.source.media_type,
            size: i.source.size,
            rendition_id: i.rendition_id.map(|r| r.0),
            duration_ms: m.map(|f| f.duration_ms),
            codec: m.map(|f| f.codec.clone()),
            sample_rate: m.map(|f| f.sample_rate),
            channels: m.map(|f| f.channels),
            gaps: m.map(|f| f.gaps),
            gap_ms: m.map(|f| f.gap_ms),
            skipped_spans: m.map(|f| f.skipped_spans),
            coarse_spans: m.map(|f| f.coarse_spans),
        }
    }
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct TranscriptQuery {
    /// Text around this time, in milliseconds. Goes with `window_ms`.
    pub around_ms: Option<i64>,
    /// How far either side of `around_ms`. The default is 30000.
    pub window_ms: Option<i64>,
    /// Text from this time on.
    pub from_ms: Option<i64>,
    /// Text up to this time.
    pub to_ms: Option<i64>,
    /// Include each word's own times.
    pub words: Option<bool>,
    /// From `next_cursor` of the previous page.
    pub cursor: Option<String>,
    /// 1 to 200. The default is 50.
    pub limit: Option<u32>,
}

impl TranscriptQuery {
    /// The window, the word to start from and the page size.
    pub fn validated(&self) -> Result<(i64, i64, usize, usize), String> {
        let limit = self.limit.unwrap_or(50);
        if !(1..=200).contains(&limit) {
            return Err(format!("limit must be from 1 to 200, not {limit}"));
        }
        let after = match &self.cursor {
            None => 0,
            Some(c) => c.parse::<usize>().map_err(|_| format!("'{c}' isn't a cursor from this list"))?,
        };
        let (from, to) = match self.around_ms {
            Some(_) if self.from_ms.is_some() || self.to_ms.is_some() => return Err("around_ms doesn't go with from_ms or to_ms".into()),
            Some(at) => {
                let window = self.window_ms.unwrap_or(30_000);
                if window < 0 {
                    return Err("window_ms can't be negative".into());
                }
                (at.saturating_sub(window), at.saturating_add(window))
            }
            None if self.window_ms.is_some() => return Err("window_ms goes with around_ms".into()),
            None => (self.from_ms.unwrap_or(i64::MIN), self.to_ms.unwrap_or(i64::MAX)),
        };
        if from > to {
            return Err("from_ms is after to_ms".into());
        }
        Ok((from, to, after, limit as usize))
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct TranscriptWordDto {
    pub text: String,
    /// The table's own times, with no padding.
    pub start_ms: i64,
    pub end_ms: i64,
}

/// A stretch of a transcript: a turn, cut to the window asked for. Its range is the same a claim citing these words gets.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct TranscriptPartDto {
    /// UTF-8 byte offsets into the rendition's canonical text.
    pub start: u64,
    pub end: u64,
    pub text: String,
    pub t_start_ms: i64,
    pub t_end_ms: i64,
    /// Who was speaking, when that's known and one person spoke all of it.
    pub speaker: Option<String>,
    pub words: Option<Vec<TranscriptWordDto>>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct TranscriptDto {
    pub rendition_id: String,
    pub parser: String,
    pub duration_ms: i64,
    pub parts: Vec<TranscriptPartDto>,
    pub next_cursor: Option<String>,
}

pub fn transcript_dto(page: TranscriptPage, with_words: bool) -> TranscriptDto {
    TranscriptDto {
        rendition_id: page.rendition_id.0,
        parser: page.parser,
        duration_ms: page.duration_ms,
        parts: page
            .parts
            .into_iter()
            .zip(page.words)
            .map(|(p, words)| TranscriptPartDto {
                start: p.start,
                end: p.end,
                text: p.text,
                t_start_ms: p.time.t_start_ms,
                t_end_ms: p.time.t_end_ms,
                speaker: p.time.speaker,
                words: with_words.then(|| {
                    words
                        .into_iter()
                        .map(|w| TranscriptWordDto {
                            text: w.text,
                            start_ms: w.start_ms,
                            end_ms: w.end_ms,
                        })
                        .collect()
                }),
            })
            .collect(),
        next_cursor: page.next.map(|n| n.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q() -> TranscriptQuery {
        TranscriptQuery {
            around_ms: None,
            window_ms: None,
            from_ms: None,
            to_ms: None,
            words: None,
            cursor: None,
            limit: None,
        }
    }

    #[test]
    fn with_nothing_asked_the_window_is_everything() {
        assert_eq!(q().validated().unwrap(), (i64::MIN, i64::MAX, 0, 50));
    }

    #[test]
    fn around_a_time_is_a_window_either_side() {
        let a = TranscriptQuery {
            around_ms: Some(60_000),
            ..q()
        };
        assert_eq!(a.validated().unwrap().0, 30_000);
        let a = TranscriptQuery {
            around_ms: Some(60_000),
            window_ms: Some(5_000),
            ..q()
        };
        assert_eq!((a.validated().unwrap().0, a.validated().unwrap().1), (55_000, 65_000));
    }

    #[test]
    fn nonsense_is_refused() {
        for bad in [
            TranscriptQuery {
                around_ms: Some(1),
                from_ms: Some(1),
                ..q()
            },
            TranscriptQuery { window_ms: Some(1), ..q() },
            TranscriptQuery {
                around_ms: Some(1),
                window_ms: Some(-1),
                ..q()
            },
            TranscriptQuery {
                from_ms: Some(5),
                to_ms: Some(1),
                ..q()
            },
            TranscriptQuery { limit: Some(0), ..q() },
            TranscriptQuery { limit: Some(201), ..q() },
            TranscriptQuery {
                cursor: Some("x".into()),
                ..q()
            },
        ] {
            assert!(bad.validated().is_err());
        }
    }
}
