//! Recordings: the `Transcriber` trait, the timed word table's canonical text, segments that follow turns and pauses, and playable ranges. All pure.
use crate::rendition::{Canonical, MAX_TOKENS, MIN_TOKENS, SegmentSpan, WordSpan, estimate_tokens, split_long};
use memex_types::{MeetingMeta, TimeSpan, TimedDocument, TimedWord, TranscribeUsage};
use serde_json::Value;
use std::fmt;
use std::ops::Range;

/// The rate a `Transcriber` hears.
pub const PCM_RATE: u32 = 16_000;
/// Word starts from the local model run a steady 150-210 ms late (spec 027), and ends are the next word's start. A cited range starts and ends this far outside its words, so it holds what was said. Set from that finding, before item 1 measured anything.
pub const START_PAD_MS: i64 = 300;
pub const END_PAD_MS: i64 = 300;
/// A gap this long between words starts a new block of the transcript.
pub const PAUSE_MS: i64 = 800;
/// A change of speaker closes a segment once it holds this much, so short interjections stay with what they answer.
pub const SPEAKER_TURN_MIN_TOKENS: usize = 30;
/// Refuse an API transcript when under this share of its words match the local clock (spec 027).
pub const MIN_RETIME_MATCH: f64 = 0.5;

const MAX_TITLE_CHARS: usize = 512;
const MAX_NAME_CHARS: usize = 256;
const MAX_PARTICIPANTS: usize = 200;

/// What a `Transcriber` heard.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Transcribed {
    /// Times are relative to the start of the audio it was given.
    pub words: Vec<TimedWord>,
    pub usage: TranscribeUsage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TranscribeError {
    /// Something it needs is away: a provider's outage or its rate limit.
    Unavailable(String),
    /// This can't work, and trying again won't change that.
    Failed(String),
}

impl fmt::Display for TranscribeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(m) | Self::Failed(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for TranscribeError {}

/// Hears speech. It takes samples and not a file or a path, so it can be local or remote, and no model's or provider's type crosses it.
pub trait Transcriber {
    /// Names the model, as part of the Rendition's parser.
    fn model(&self) -> String;
    /// `pcm` is mono at `PCM_RATE`. `language` is a hint such as `sv`, and None leaves it to the model.
    fn transcribe(&self, pcm: &[f32], language: Option<&str>) -> Result<Transcribed, TranscribeError>;
}

/// Audio and video, by the media type we sniffed.
pub fn is_media(media_type: &str) -> bool {
    media_type.starts_with("audio/") || media_type.starts_with("video/")
}

/// Words as a table should hold them: non-empty, in time order, with a start at or after the one before and an end at or after its start, and all inside the recording.
pub fn clean_words(words: Vec<TimedWord>, duration_ms: i64) -> Vec<TimedWord> {
    let mut out: Vec<TimedWord> = Vec::with_capacity(words.len());
    for mut w in words {
        w.text = w.text.trim().to_string();
        if w.text.is_empty() {
            continue;
        }
        let floor = out.last().map_or(0, |p| p.start_ms);
        w.start_ms = w.start_ms.max(floor).min(duration_ms);
        w.end_ms = w.end_ms.max(w.start_ms).min(duration_ms);
        out.push(w);
    }
    out
}

/// A word's end is the next word's start, capped at 1.5 s, since the local model's timing gives starts only (spec 027). The end it reported stands when that is earlier than the next start and the cap doesn't apply.
pub fn close_word_ends(words: &mut [TimedWord]) {
    const CAP_MS: i64 = 1500;
    for i in 0..words.len().saturating_sub(1) {
        let (start, next) = (words[i].start_ms, words[i + 1].start_ms);
        if next > start && next - start < CAP_MS {
            words[i].end_ms = next;
        } else {
            words[i].end_ms = words[i].end_ms.min(next).min(start + CAP_MS).max(start);
        }
    }
}

fn boundary(previous: &TimedWord, next: &TimedWord) -> bool {
    previous.speaker != next.speaker || next.start_ms - previous.end_ms >= PAUSE_MS
}

/// Words in runs with no change of speaker and no pause.
pub fn turns(words: &[TimedWord]) -> Vec<Range<usize>> {
    let (mut out, mut from) = (vec![], 0);
    for i in 1..words.len() {
        if boundary(&words[i - 1], &words[i]) {
            out.push(from..i);
            from = i;
        }
    }
    if from < words.len() {
        out.push(from..words.len());
    }
    out
}

/// The canonical text: words with spaces, and a blank line at each change of speaker or pause. The same words always give the same bytes. Speakers aren't in it, as a quote has to be what was said.
pub fn canonical_timed(words: &[TimedWord]) -> Canonical {
    let mut text = String::new();
    let mut spans = Vec::with_capacity(words.len());
    for (i, w) in words.iter().enumerate() {
        if i > 0 {
            text.push_str(if boundary(&words[i - 1], w) { "\n\n" } else { " " });
        }
        let start = text.len();
        text.push_str(&w.text);
        spans.push(WordSpan { start, end: text.len() });
    }
    Canonical { text, spans }
}

/// The speaker when one person spoke all of these words.
fn one_speaker(words: &[TimedWord]) -> Option<String> {
    let first = words.first()?.speaker.as_ref()?;
    words.iter().all(|w| w.speaker.as_ref() == Some(first)).then(|| first.clone())
}

/// The range to play for these words, inside the recording. This is the one place a word's times become a cited range, so the transcript and the provenance API can't disagree.
pub fn play_range(words: &[TimedWord], duration_ms: i64) -> Option<TimeSpan> {
    let start = words.iter().map(|w| w.start_ms).min()?;
    let end = words.iter().map(|w| w.end_ms).max()?;
    Some(TimeSpan {
        t_start_ms: (start - START_PAD_MS).max(0),
        t_end_ms: (end + END_PAD_MS).min(duration_ms.max(end)),
        speaker: one_speaker(words),
    })
}

/// The words a byte range of the canonical text touches.
pub fn words_in(spans: &[WordSpan], start: usize, end: usize) -> Range<usize> {
    let from = spans.partition_point(|s| s.end <= start);
    let to = spans.partition_point(|s| s.start < end);
    from..to.max(from)
}

/// A recording's word table with its text, for finding times from byte offsets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimedIndex {
    pub doc: TimedDocument,
    pub canon: Canonical,
}

impl TimedIndex {
    pub fn new(doc: TimedDocument) -> Self {
        let canon = canonical_timed(&doc.words);
        Self { doc, canon }
    }

    /// The range to play for a byte range of the canonical text. None when it touches no word.
    pub fn span_of(&self, start: u64, end: u64) -> Option<TimeSpan> {
        let range = words_in(&self.canon.spans, start as usize, end as usize);
        play_range(&self.doc.words[range], self.doc.media.duration_ms)
    }
}

/// A segment of a recording, with where it plays.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimedSegment {
    pub span: SegmentSpan,
    pub time: Option<TimeSpan>,
}

/// Segments of about 100 to 400 tokens that end at a change of speaker or a pause. Only a turn over the maximum is cut inside itself, at a sentence end where it can be.
pub fn timed_segments(canon: &Canonical, words: &[TimedWord], duration_ms: i64) -> Vec<TimedSegment> {
    let text = &canon.text;
    let mut pieces: Vec<(usize, usize)> = vec![];
    let mut current: Option<Range<usize>> = None;
    let tokens = |r: &Range<usize>| estimate_tokens(&text[canon.spans[r.start].start..canon.spans[r.end - 1].end]);
    for turn in turns(words) {
        let Some(open) = current.clone() else {
            current = Some(turn);
            continue;
        };
        let held = tokens(&open);
        let speaker_changed = words[open.end - 1].speaker != words[turn.start].speaker && held >= SPEAKER_TURN_MIN_TOKENS;
        if held >= MIN_TOKENS || speaker_changed || held + tokens(&turn) > MAX_TOKENS {
            pieces.push((canon.spans[open.start].start, canon.spans[open.end - 1].end));
            current = Some(turn);
        } else {
            current = Some(open.start..turn.end);
        }
    }
    if let Some(open) = current {
        pieces.push((canon.spans[open.start].start, canon.spans[open.end - 1].end));
    }
    let mut out = vec![];
    for (a, b) in pieces {
        for (start, end) in split_long(text, a, b, MAX_TOKENS) {
            let range = words_in(&canon.spans, start, end);
            out.push(TimedSegment {
                span: SegmentSpan {
                    start,
                    end,
                    heading: None,
                    page_from: None,
                    page_to: None,
                },
                time: play_range(&words[range], duration_ms),
            });
        }
    }
    out
}

/// A stretch of a transcript.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptPart {
    /// UTF-8 byte offsets into the canonical text.
    pub start: u64,
    pub end: u64,
    pub text: String,
    pub time: TimeSpan,
    pub words: Range<usize>,
}

/// The transcript's turns, each clipped to the words overlapping `[from_ms, to_ms]`, in time order and starting at word `after`. A word's own times decide, so asking around a moment inside a long turn doesn't return all of the turn.
pub fn transcript_parts(index: &TimedIndex, from_ms: i64, to_ms: i64, after: usize, limit: usize) -> (Vec<TranscriptPart>, Option<usize>) {
    let words = &index.doc.words;
    let overlaps = |w: &TimedWord| w.end_ms >= from_ms && w.start_ms <= to_ms;
    let mut out = vec![];
    for turn in turns(words) {
        let Some(first) = turn.clone().find(|&i| i >= after && overlaps(&words[i])) else {
            continue;
        };
        let Some(last) = turn.clone().rev().find(|&i| overlaps(&words[i])) else {
            continue;
        };
        if out.len() == limit {
            return (out, Some(first));
        }
        let (start, end) = (index.canon.spans[first].start, index.canon.spans[last].end);
        let Some(time) = play_range(&words[first..=last], index.doc.media.duration_ms) else {
            continue;
        };
        out.push(TranscriptPart {
            start: start as u64,
            end: end as u64,
            text: index.canon.text[start..end].to_string(),
            time,
            words: first..last + 1,
        });
    }
    (out, None)
}

fn norm(word: &str) -> String {
    word.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// Places an API's words on the local model's clock. Runs of at least 2 equal words take the clock's times, as a lone match is often chance, and the words between runs share the time between them by length. Returns the share of words matched. Spec 027 found the API's own times unusable, and this moves its words by up to 80 s.
pub fn retime(api: &mut [TimedWord], clock: &[TimedWord]) -> f64 {
    let a: Vec<String> = api.iter().map(|w| norm(&w.text)).collect();
    let c: Vec<String> = clock.iter().map(|w| norm(&w.text)).collect();
    let mut matched = vec![false; api.len()];
    let mut count = 0usize;
    for op in similar::capture_diff_slices(similar::Algorithm::Myers, &a, &c) {
        if let similar::DiffOp::Equal { old_index, new_index, len } = op
            && len >= 2
        {
            for k in 0..len {
                api[old_index + k].start_ms = clock[new_index + k].start_ms;
                api[old_index + k].end_ms = clock[new_index + k].end_ms;
                matched[old_index + k] = true;
                count += 1;
            }
        }
    }
    let anchors: Vec<usize> = (0..api.len()).filter(|&i| matched[i]).collect();
    for pair in anchors.windows(2) {
        let (lo, hi) = (pair[0], pair[1]);
        if hi - lo < 2 {
            continue;
        }
        let (t0, t1) = (api[lo].end_ms, api[hi].start_ms.max(api[lo].end_ms));
        let total: i64 = (lo + 1..hi).map(|i| a[i].chars().count().max(1) as i64).sum();
        let mut used = 0i64;
        for i in lo + 1..hi {
            api[i].start_ms = t0 + (t1 - t0) * used / total;
            used += a[i].chars().count().max(1) as i64;
            api[i].end_ms = t0 + (t1 - t0) * used / total;
        }
    }
    // Before the first run and after the last there's no clock to go by, so these words keep their lengths and stay next to it.
    if let (Some(&first), Some(&last)) = (anchors.first(), anchors.last()) {
        for i in (0..first).rev() {
            let len = (api[i].end_ms - api[i].start_ms).clamp(0, 2000);
            api[i].end_ms = api[i + 1].start_ms;
            api[i].start_ms = api[i].end_ms - len;
        }
        for i in last + 1..api.len() {
            let len = (api[i].end_ms - api[i].start_ms).clamp(0, 2000);
            api[i].start_ms = api[i - 1].end_ms;
            api[i].end_ms = api[i].start_ms + len;
        }
    }
    if api.is_empty() { 1.0 } else { count as f64 / api.len() as f64 }
}

/// A recording's meeting metadata, from its Source's metadata.
pub fn meeting_of(metadata: &Value) -> MeetingMeta {
    metadata.get("meeting").and_then(|m| serde_json::from_value(m.clone()).ok()).unwrap_or_default()
}

/// The Source's metadata with this meeting in it. Anything else there stays.
pub fn with_meeting(metadata: &Value, meeting: &MeetingMeta) -> Value {
    let mut out = match metadata {
        Value::Object(m) => m.clone(),
        _ => serde_json::Map::new(),
    };
    out.insert("meeting".into(), serde_json::to_value(meeting).unwrap_or(Value::Null));
    Value::Object(out)
}

/// The language a recording was uploaded with, if any.
pub fn language_of(metadata: &Value) -> Option<String> {
    metadata.get("language").and_then(Value::as_str).map(str::to_string)
}

/// A language hint is a short lowercase code, which also keeps it safe to pass to a worker.
pub fn valid_language(code: &str) -> bool {
    (2..=3).contains(&code.len()) && code.chars().all(|c| c.is_ascii_lowercase())
}

pub fn check_meeting(meeting: &MeetingMeta) -> Result<(), String> {
    if meeting.title.as_ref().is_some_and(|t| t.chars().count() > MAX_TITLE_CHARS) {
        return Err(format!("a title can be at most {MAX_TITLE_CHARS} characters"));
    }
    if meeting.participants.len() > MAX_PARTICIPANTS {
        return Err(format!("a meeting can have at most {MAX_PARTICIPANTS} participants"));
    }
    for (i, p) in meeting.participants.iter().enumerate() {
        if p.name.trim().is_empty() || p.name.chars().count() > MAX_NAME_CHARS {
            return Err(format!("a participant's name must be 1 to {MAX_NAME_CHARS} characters"));
        }
        if meeting.participants[..i].iter().any(|q| !p.id.is_empty() && q.id == p.id) {
            return Err(format!("participant id {} appears twice", p.id));
        }
    }
    Ok(())
}

/// The Edge saying a participant is this Person. Its id comes from the recording and the participant, so a person has one link, a new version of the recording keeps it, and a read finds it with one batched lookup.
pub fn participant_edge_id(logical_source: &str, participant: &str) -> memex_types::ObjectId {
    memex_types::ObjectId(format!("participant-{logical_source}-{participant}"))
}

pub const PARTICIPANT_EDGE: &str = "meeting_participant";

#[cfg(test)]
mod tests {
    use super::*;
    use memex_types::MediaFacts;

    fn w(text: &str, start_ms: i64, end_ms: i64) -> TimedWord {
        TimedWord {
            text: text.into(),
            start_ms,
            end_ms,
            speaker: None,
        }
    }

    fn sw(text: &str, start_ms: i64, speaker: &str) -> TimedWord {
        TimedWord {
            speaker: Some(speaker.into()),
            ..w(text, start_ms, start_ms + 300)
        }
    }

    fn index(words: Vec<TimedWord>, duration_ms: i64) -> TimedIndex {
        TimedIndex::new(TimedDocument {
            parser: "t".into(),
            media: MediaFacts {
                duration_ms,
                ..Default::default()
            },
            words,
            usage: TranscribeUsage::default(),
        })
    }

    #[test]
    fn clean_words_orders_clamps_and_drops_empties() {
        let c = clean_words(vec![w(" a ", 100, 50), w("  ", 200, 300), w("b", 90, 95), w("c", 900, 5000)], 1000);
        assert_eq!(c, vec![w("a", 100, 100), w("b", 100, 100), w("c", 900, 1000)]);
    }

    #[test]
    fn a_word_ends_at_the_next_start_with_a_cap() {
        let mut words = vec![w("a", 0, 10), w("b", 400, 410), w("c", 5000, 5900), w("d", 5400, 5500)];
        close_word_ends(&mut words);
        assert_eq!(words[0].end_ms, 400);
        assert_eq!(words[1].end_ms, 410, "a long gap keeps the end it had");
        assert_eq!(words[2].end_ms, 5400);
        assert_eq!(words[3].end_ms, 5500, "the last word keeps its own");
    }

    #[test]
    fn a_reported_end_past_the_next_start_is_cut_back() {
        let mut words = vec![w("a", 0, 9000), w("b", 4000, 4100)];
        close_word_ends(&mut words);
        assert_eq!(words[0].end_ms, 1500);
    }

    #[test]
    fn a_blank_line_marks_each_pause_and_change_of_speaker() {
        let words = vec![sw("hej", 0, "A"), sw("du", 300, "A"), sw("ja", 600, "B"), sw("nej", 5000, "B")];
        let c = canonical_timed(&words);
        assert_eq!(c.text, "hej du\n\nja\n\nnej");
        for (s, word) in c.spans.iter().zip(&words) {
            assert_eq!(&c.text[s.start..s.end], word.text);
        }
        assert_eq!(turns(&words), vec![0..2, 2..3, 3..4]);
    }

    #[test]
    fn nothing_said_gives_no_text_and_no_segments() {
        let c = canonical_timed(&[]);
        assert_eq!(c.text, "");
        assert!(timed_segments(&c, &[], 1000).is_empty());
        assert!(turns(&[]).is_empty());
    }

    #[test]
    fn a_range_has_its_padding_stays_inside_the_recording_and_names_one_speaker_only() {
        let words = vec![sw("a", 100, "A"), sw("b", 400, "A")];
        let r = play_range(&words, 600).unwrap();
        assert_eq!((r.t_start_ms, r.t_end_ms, r.speaker.as_deref()), (0, 700.min(600).max(700), Some("A")));
        let mixed = vec![sw("a", 1000, "A"), sw("b", 1400, "B")];
        let r = play_range(&mixed, 100_000).unwrap();
        assert_eq!((r.t_start_ms, r.t_end_ms, r.speaker), (700, 2000, None));
        assert_eq!(play_range(&[], 100), None);
    }

    #[test]
    fn a_range_never_ends_before_the_last_word_even_on_a_short_duration() {
        let r = play_range(&[w("a", 1000, 1100)], 1050).unwrap();
        assert!(r.t_end_ms >= 1100);
    }

    #[test]
    fn a_byte_range_finds_its_words() {
        let i = index(vec![w("hej", 1000, 1500), w("på", 1500, 1800), w("dig", 1800, 2500)], 10_000);
        assert_eq!(i.canon.text, "hej på dig");
        let (a, b) = crate::rendition::find_quote(&i.canon.text, "på dig", "", "").unwrap();
        let t = i.span_of(a as u64, b as u64).unwrap();
        assert_eq!((t.t_start_ms, t.t_end_ms), (1200, 2800));
        // Part of a word counts as the word.
        assert_eq!(i.span_of(1, 2).unwrap().t_start_ms, 700);
        assert_eq!(i.span_of(99, 100), None);
    }

    #[test]
    fn segments_end_at_turns_and_pauses_when_they_can() {
        let mut words = vec![];
        let mut t = 0;
        for block in 0..8 {
            for i in 0..50 {
                words.push(w(&format!("b{block}w{i}"), t, t + 200));
                t += 200;
            }
            t += 2000;
        }
        let c = canonical_timed(&words);
        let segs = timed_segments(&c, &words, t);
        assert!(segs.len() >= 3, "{}", segs.len());
        for s in &segs {
            assert!(
                c.text[..s.span.start].is_empty() || c.text[..s.span.start].ends_with("\n\n"),
                "starts at a block"
            );
            assert!(s.time.is_some());
        }
        let joined: Vec<&str> = segs.iter().flat_map(|s| c.text[s.span.start..s.span.end].split_whitespace()).collect();
        assert_eq!(joined, c.text.split_whitespace().collect::<Vec<_>>(), "every word in exactly one segment");
        for pair in segs.windows(2) {
            assert!(pair[0].time.as_ref().unwrap().t_start_ms <= pair[1].time.as_ref().unwrap().t_start_ms);
        }
    }

    #[test]
    fn a_turn_over_the_maximum_is_cut_inside_itself() {
        let words: Vec<TimedWord> = (0..1500).map(|i| w(&format!("w{i}."), i * 100, i * 100 + 100)).collect();
        let c = canonical_timed(&words);
        let segs = timed_segments(&c, &words, 200_000);
        assert!(segs.len() >= 4);
        for s in &segs {
            assert!(estimate_tokens(&c.text[s.span.start..s.span.end]) <= MAX_TOKENS + 4);
        }
    }

    #[test]
    fn a_change_of_speaker_closes_a_segment_that_holds_enough() {
        let mut words = vec![];
        for i in 0..40 {
            words.push(sw(&format!("a{i}"), i * 300, "A"));
        }
        for i in 40..80 {
            words.push(sw(&format!("b{i}"), i * 300, "B"));
        }
        let c = canonical_timed(&words);
        let segs = timed_segments(&c, &words, 100_000);
        assert_eq!(segs.len(), 2);
        assert_eq!(segs[0].time.as_ref().unwrap().speaker.as_deref(), Some("A"));
        assert_eq!(segs[1].time.as_ref().unwrap().speaker.as_deref(), Some("B"));
    }

    #[test]
    fn a_short_interjection_stays_with_what_it_answers() {
        let words = vec![sw("ja", 0, "A"), sw("okej", 300, "B"), sw("bra", 600, "A")];
        let c = canonical_timed(&words);
        let segs = timed_segments(&c, &words, 2000);
        assert_eq!(segs.len(), 1);
        assert_eq!(segs[0].time.as_ref().unwrap().speaker, None);
    }

    #[test]
    fn transcript_parts_clip_to_the_window_and_page() {
        let i = index(
            vec![w("a", 0, 500), w("b", 500, 1000), w("c", 1000, 1500), w("d", 9000, 9500), w("e", 9500, 9900)],
            20_000,
        );
        let (parts, next) = transcript_parts(&i, 700, 1200, 0, 10);
        assert_eq!(next, None);
        assert_eq!(parts.len(), 1);
        assert_eq!(parts[0].text, "b c");
        assert_eq!(parts[0].words, 1..3);
        assert_eq!(&i.canon.text[parts[0].start as usize..parts[0].end as usize], "b c");
        assert_eq!((parts[0].time.t_start_ms, parts[0].time.t_end_ms), (200, 1800));
        let (all, next) = transcript_parts(&i, i64::MIN, i64::MAX, 0, 1);
        assert_eq!(all[0].text, "a b c");
        let next = next.unwrap();
        let (rest, end) = transcript_parts(&i, i64::MIN, i64::MAX, next, 1);
        assert_eq!((rest[0].text.as_str(), end), ("d e", None));
    }

    #[test]
    fn a_transcript_part_agrees_with_the_range_of_the_same_bytes() {
        let i = index(vec![w("a", 0, 500), w("b", 500, 1000), w("c", 9000, 9500)], 20_000);
        for p in transcript_parts(&i, i64::MIN, i64::MAX, 0, 10).0 {
            assert_eq!(i.span_of(p.start, p.end).as_ref(), Some(&p.time));
        }
    }

    #[test]
    fn retiming_takes_the_clock_and_interpolates_between_runs() {
        let clock = vec![
            w("the", 100, 200),
            w("pump", 200, 300),
            w("was", 300, 400),
            w("moved", 400, 500),
            w("north", 500, 900),
            w("today", 900, 1000),
            w("again", 1000, 1100),
        ];
        let mut api = vec![
            w("The", 5000, 5100),
            w("pump", 5100, 5200),
            w("WAS", 5200, 5300),
            w("carried", 5300, 5400),
            w("north,", 5400, 5500),
            w("today", 5500, 5600),
            w("again.", 5600, 5700),
        ];
        let share = retime(&mut api, &clock);
        assert!((share - 6.0 / 7.0).abs() < 1e-9, "{share}");
        assert_eq!((api[0].start_ms, api[2].end_ms), (100, 400));
        assert_eq!(api[4].start_ms, 500);
        assert_eq!((api[3].start_ms, api[3].end_ms), (400, 500), "between two runs, on the clock");
    }

    #[test]
    fn a_lone_equal_word_does_not_count_as_a_match() {
        let clock = vec![w("x", 10, 20), w("the", 20, 30), w("y", 30, 40)];
        let mut api = vec![w("a", 900, 950), w("the", 950, 990), w("b", 990, 999)];
        assert_eq!(retime(&mut api, &clock), 0.0);
        assert_eq!(api[1].start_ms, 950, "with no run, nothing moves");
    }

    #[test]
    fn retiming_keeps_words_outside_the_runs_next_to_them() {
        let clock = vec![w("one", 1000, 1100), w("two", 1100, 1200)];
        let mut api = vec![w("zero", 0, 300), w("one", 300, 400), w("two", 400, 500), w("three", 500, 800)];
        retime(&mut api, &clock);
        assert_eq!((api[0].start_ms, api[0].end_ms), (700, 1000));
        assert_eq!((api[3].start_ms, api[3].end_ms), (1200, 1500));
    }

    #[test]
    fn nothing_to_retime_is_not_a_refusal() {
        assert_eq!(retime(&mut [], &[w("a", 0, 1)]), 1.0);
    }

    #[test]
    fn meeting_metadata_round_trips_and_keeps_what_else_is_there() {
        let meeting = MeetingMeta {
            title: Some("Veckomöte".into()),
            date_ms: Some(5),
            participants: vec![memex_types::Participant {
                id: "p1".into(),
                name: "Åsa".into(),
                speaker: Some("S1".into()),
            }],
        };
        let m = with_meeting(&serde_json::json!({"language": "sv"}), &meeting);
        assert_eq!(meeting_of(&m), meeting);
        assert_eq!(language_of(&m).as_deref(), Some("sv"));
        assert_eq!(meeting_of(&serde_json::json!({})), MeetingMeta::default());
        assert_eq!(meeting_of(&serde_json::json!({"meeting": 7})), MeetingMeta::default());
    }

    #[test]
    fn meeting_limits_hold() {
        let p = |id: &str, name: &str| memex_types::Participant {
            id: id.into(),
            name: name.into(),
            speaker: None,
        };
        let m = |participants| MeetingMeta {
            participants,
            ..Default::default()
        };
        assert!(check_meeting(&m(vec![p("a", "A"), p("b", "B")])).is_ok());
        assert!(check_meeting(&m(vec![p("a", " ")])).is_err());
        assert!(check_meeting(&m(vec![p("a", "A"), p("a", "B")])).is_err());
        assert!(check_meeting(&m(vec![p("a", "A"); 201])).is_err());
        assert!(
            check_meeting(&MeetingMeta {
                title: Some("x".repeat(513)),
                ..Default::default()
            })
            .is_err()
        );
    }

    #[test]
    fn language_hints_are_short_codes() {
        for ok in ["sv", "en", "yue"] {
            assert!(valid_language(ok));
        }
        for bad in ["", "s", "SV", "svenska", "s v", "../"] {
            assert!(!valid_language(bad), "{bad}");
        }
    }

    #[test]
    fn audio_and_video_are_media() {
        assert!(is_media("audio/ogg") && is_media("video/mp4"));
        assert!(!is_media("application/pdf") && !is_media("text/plain"));
    }
}
