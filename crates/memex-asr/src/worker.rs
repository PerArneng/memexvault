//! The transcription worker's work, in a folder that survives it. whisper.cpp can abort the process, so each stage writes what it has done before and after, and a new process carries on from there.
use crate::decode::{DecodeError, decode_to_file};
use memex_core::meeting::{MIN_RETIME_MATCH, PCM_RATE, clean_words, retime};
use memex_core::{TranscribeError, Transcriber};
use memex_types::{MediaFacts, TimedDocument, TimedWord, TranscribeUsage};
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

pub const SOURCE_FILE: &str = "source";
pub const RESULT_FILE: &str = "result.json";
const PCM_FILE: &str = "pcm.f32";
const FACTS_FILE: &str = "facts.json";
const SPANS_FILE: &str = "spans.json";
const PROGRESS_FILE: &str = "progress.jsonl";
const API_FILE: &str = "api.jsonl";

/// A worker exits with this after refusing a file, with the reason on stderr.
pub const EXIT_REFUSED: u8 = 3;
/// A worker exits with this when a provider is away, so the job waits.
pub const EXIT_WAITING: u8 = 4;

/// How much audio goes to the API in one call: about 5 minutes, cut at the quietest point of its last 30 s, so no word is cut in half (spec 027).
const API_CHUNK_SECONDS: u64 = 300;
const API_CUT_SEARCH_SECONDS: u64 = 30;

/// Why a worker stopped without a result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkerFail {
    /// The recording can't be used, and trying again won't help.
    Refused(String),
    /// A provider is away.
    Waiting(String),
    /// Something else, which may pass.
    Failed(String),
}

impl WorkerFail {
    pub fn io(e: std::io::Error) -> Self {
        Self::Failed(e.to_string())
    }

    pub fn exit_code(&self) -> u8 {
        match self {
            Self::Refused(_) => EXIT_REFUSED,
            Self::Waiting(_) => EXIT_WAITING,
            Self::Failed(_) => 1,
        }
    }

    pub fn message(&self) -> &str {
        match self {
            Self::Refused(m) | Self::Waiting(m) | Self::Failed(m) => m,
        }
    }
}

fn json_error(e: serde_json::Error) -> WorkerFail {
    WorkerFail::Failed(e.to_string())
}

fn from_transcriber(e: TranscribeError) -> WorkerFail {
    match e {
        TranscribeError::Unavailable(m) => WorkerFail::Waiting(m),
        TranscribeError::Failed(m) => WorkerFail::Failed(m),
    }
}

/// Stretch of the recording, in samples at 16 kHz.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Span {
    pub start: u64,
    pub end: u64,
}

/// The decoded recording on disk. Only what's asked for comes into memory.
pub struct PcmFile {
    file: std::cell::RefCell<File>,
    samples: u64,
}

impl PcmFile {
    pub fn open(path: &Path) -> std::io::Result<Self> {
        let file = File::open(path)?;
        let samples = file.metadata()?.len() / 4;
        Ok(Self {
            file: std::cell::RefCell::new(file),
            samples,
        })
    }

    pub fn samples(&self) -> u64 {
        self.samples
    }

    pub fn read(&self, start: u64, count: u64) -> std::io::Result<Vec<f32>> {
        let count = count.min(self.samples.saturating_sub(start));
        let mut bytes = vec![0u8; count as usize * 4];
        let mut f = self.file.borrow_mut();
        f.seek(SeekFrom::Start(start * 4))?;
        f.read_exact(&mut bytes)?;
        Ok(bytes.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect())
    }
}

/// What finds speech and hears it. Whisper's in production. The contexts load only when a stretch needs one, so a crashed stretch's fallback isn't in memory beside the usual context.
pub trait Ears {
    type T: Transcriber;
    /// Where speech is. Whisper repeats phrases in silence without this, and `state.full` ignores its own VAD setting, so it's a step of its own (spec 027).
    fn speech_spans(&self, pcm: &PcmFile) -> Result<Vec<Span>, WorkerFail>;
    fn precise(&self) -> Result<Self::T, WorkerFail>;
    /// For a stretch where `precise` crashed the process: less accurate times, and it doesn't crash that way.
    fn coarse(&self) -> Result<Self::T, WorkerFail>;
}

/// The API's part, when settings turned it on. Without this no audio leaves the machine: nothing else in this file can send anything.
pub struct ApiPart<'a, A: Transcriber> {
    pub transcriber: &'a A,
    /// Spending for one recording stops here.
    pub max_tokens: u64,
}

#[derive(Debug, Serialize, Deserialize)]
struct Line {
    i: usize,
    /// Absent on the line written before a stretch starts, so a stretch with a start and no words crashed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    words: Option<Vec<TimedWord>>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    coarse: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    skipped: bool,
    #[serde(default)]
    usage: TranscribeUsage,
}

fn append(path: &Path, line: &Line) -> Result<(), WorkerFail> {
    let mut f = OpenOptions::new().create(true).append(true).open(path).map_err(WorkerFail::io)?;
    let mut text = serde_json::to_vec(line).map_err(json_error)?;
    text.push(b'\n');
    f.write_all(&text).map_err(WorkerFail::io)?;
    f.sync_data().map_err(WorkerFail::io)
}

/// What a log holds. A crash can leave its last line cut short, and that one counts as not written.
fn read_lines(path: &Path) -> Result<Vec<Line>, WorkerFail> {
    let text = match fs::read_to_string(path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(WorkerFail::io(e)),
    };
    Ok(text.lines().filter_map(|l| serde_json::from_str(l).ok()).collect())
}

fn write_atomic(dir: &Path, name: &str, bytes: &[u8]) -> Result<(), WorkerFail> {
    let temporary = dir.join(format!("{name}.part"));
    let mut f = File::create(&temporary).map_err(WorkerFail::io)?;
    f.write_all(bytes).map_err(WorkerFail::io)?;
    f.sync_data().map_err(WorkerFail::io)?;
    fs::rename(&temporary, dir.join(name)).map_err(WorkerFail::io)
}

fn ms_of(sample: u64) -> i64 {
    (sample * 1000 / u64::from(PCM_RATE)) as i64
}

fn shifted(words: Vec<TimedWord>, by_ms: i64) -> Vec<TimedWord> {
    words
        .into_iter()
        .map(|mut w| {
            w.start_ms += by_ms;
            w.end_ms += by_ms;
            w
        })
        .collect()
}

/// Step 1: the recording as PCM, once.
fn decoded(dir: &Path, max_seconds: u64) -> Result<(MediaFacts, PcmFile), WorkerFail> {
    let facts_path = dir.join(FACTS_FILE);
    let pcm_path = dir.join(PCM_FILE);
    if let Ok(text) = fs::read(&facts_path) {
        let facts = serde_json::from_slice(&text).map_err(json_error)?;
        return Ok((facts, PcmFile::open(&pcm_path).map_err(WorkerFail::io)?));
    }
    let partial = dir.join("pcm.part");
    let facts = decode_to_file(&dir.join(SOURCE_FILE), &partial, max_seconds).map_err(|e| match e {
        DecodeError::Refused(m) => WorkerFail::Refused(m),
        DecodeError::Io(m) => WorkerFail::Failed(m),
    })?;
    fs::rename(&partial, &pcm_path).map_err(WorkerFail::io)?;
    write_atomic(dir, FACTS_FILE, &serde_json::to_vec(&facts).map_err(json_error)?)?;
    Ok((facts, PcmFile::open(&pcm_path).map_err(WorkerFail::io)?))
}

/// Step 2: where the speech is, once.
fn spans_of<E: Ears>(dir: &Path, ears: &E, pcm: &PcmFile) -> Result<Vec<Span>, WorkerFail> {
    if let Ok(text) = fs::read(dir.join(SPANS_FILE)) {
        return serde_json::from_slice(&text).map_err(json_error);
    }
    let spans = ears.speech_spans(pcm)?;
    write_atomic(dir, SPANS_FILE, &serde_json::to_vec(&spans).map_err(json_error)?)?;
    Ok(spans)
}

/// Step 3: every stretch of speech, with saved progress. A stretch that crashed once runs again on token times, and one that crashed on those too is skipped and counted.
fn heard<E: Ears>(dir: &Path, ears: &E, pcm: &PcmFile, spans: &[Span], language: Option<&str>, facts: &mut MediaFacts) -> Result<Vec<TimedWord>, WorkerFail> {
    let path = dir.join(PROGRESS_FILE);
    let lines = read_lines(&path)?;
    let mut starts = vec![0u32; spans.len()];
    let mut done: Vec<Option<Vec<TimedWord>>> = vec![None; spans.len()];
    for line in lines {
        let Some(slot) = done.get_mut(line.i) else { continue };
        match line.words {
            Some(words) => {
                facts.coarse_spans += u32::from(line.coarse);
                facts.skipped_spans += u32::from(line.skipped);
                *slot = Some(words);
            }
            None => starts[line.i] += 1,
        }
    }
    let mut precise: Option<E::T> = None;
    for (i, span) in spans.iter().enumerate() {
        if done[i].is_some() {
            continue;
        }
        let base = ms_of(span.start);
        let mut record = Line {
            i,
            words: None,
            coarse: false,
            skipped: false,
            usage: TranscribeUsage::default(),
        };
        if starts[i] >= 2 {
            record.words = Some(vec![]);
            record.skipped = true;
            facts.skipped_spans += 1;
            append(&path, &record)?;
            done[i] = Some(vec![]);
            continue;
        }
        append(&path, &record)?;
        let audio = pcm.read(span.start, span.end - span.start).map_err(WorkerFail::io)?;
        let words = if starts[i] == 1 {
            record.coarse = true;
            facts.coarse_spans += 1;
            // Loaded and dropped here, so this process never holds it beside the usual context.
            let fallback = ears.coarse()?;
            fallback.transcribe(&audio, language).map_err(from_transcriber)?.words
        } else {
            if precise.is_none() {
                precise = Some(ears.precise()?);
            }
            precise
                .as_ref()
                .expect("set above")
                .transcribe(&audio, language)
                .map_err(from_transcriber)?
                .words
        };
        let words = shifted(words, base);
        record.words = Some(words.clone());
        append(&path, &record)?;
        done[i] = Some(words);
    }
    Ok(done.into_iter().flatten().flatten().collect())
}

/// Cut points for the API, each at the quietest 200 ms of the search window before about every 5 minutes.
fn api_chunks(pcm: &PcmFile) -> Result<Vec<Span>, WorkerFail> {
    let rate = u64::from(PCM_RATE);
    let (target, search, hop) = (API_CHUNK_SECONDS * rate, API_CUT_SEARCH_SECONDS * rate, rate / 5);
    let (mut out, mut at) = (vec![], 0u64);
    while pcm.samples() - at > target + search {
        let from = at + target - search;
        let window = pcm.read(from, search).map_err(WorkerFail::io)?;
        let (mut best, mut best_energy) = (at + target, f32::MAX);
        for (k, part) in window.chunks_exact(hop as usize).enumerate() {
            let energy: f32 = part.iter().map(|v| v * v).sum();
            if energy < best_energy {
                best_energy = energy;
                best = from + k as u64 * hop + hop / 2;
            }
        }
        out.push(Span { start: at, end: best });
        at = best;
    }
    out.push(Span { start: at, end: pcm.samples() });
    Ok(out)
}

/// Step 4, only when the API is on: its text and speakers, placed on the local clock. A chunk with no speech is never sent. Speaker labels restart each chunk (spec 027), so a label names its chunk too.
fn api_words<A: Transcriber>(
    dir: &Path,
    api: &ApiPart<'_, A>,
    pcm: &PcmFile,
    spans: &[Span],
    clock: &[TimedWord],
    language: Option<&str>,
    usage: &mut TranscribeUsage,
) -> Result<Vec<TimedWord>, WorkerFail> {
    let path = dir.join(API_FILE);
    let chunks = api_chunks(pcm)?;
    let mut done: Vec<Option<Vec<TimedWord>>> = vec![None; chunks.len()];
    for line in read_lines(&path)? {
        if let (Some(slot), Some(words)) = (done.get_mut(line.i), line.words) {
            usage.input_tokens += line.usage.input_tokens;
            usage.output_tokens += line.usage.output_tokens;
            usage.calls += line.usage.calls;
            *slot = Some(words);
        }
    }
    for (i, chunk) in chunks.iter().enumerate() {
        if done[i].is_some() || !spans.iter().any(|s| s.start < chunk.end && chunk.start < s.end) {
            continue;
        }
        if usage.input_tokens + usage.output_tokens > api.max_tokens {
            return Err(WorkerFail::Refused(format!(
                "the token limit of {} for one recording was reached",
                api.max_tokens
            )));
        }
        let audio = pcm.read(chunk.start, chunk.end - chunk.start).map_err(WorkerFail::io)?;
        let reply = api.transcriber.transcribe(&audio, language).map_err(from_transcriber)?;
        let base = ms_of(chunk.start);
        let words: Vec<TimedWord> = shifted(reply.words, base)
            .into_iter()
            .map(|mut w| {
                w.speaker = w.speaker.map(|s| format!("{base}:{s}"));
                w
            })
            .collect();
        usage.input_tokens += reply.usage.input_tokens;
        usage.output_tokens += reply.usage.output_tokens;
        usage.calls += reply.usage.calls;
        append(
            &path,
            &Line {
                i,
                words: Some(words.clone()),
                coarse: false,
                skipped: false,
                usage: reply.usage,
            },
        )?;
        done[i] = Some(words);
    }
    let mut words: Vec<TimedWord> = done.into_iter().flatten().flatten().collect();
    let share = retime(&mut words, clock);
    if share < MIN_RETIME_MATCH {
        return Err(WorkerFail::Refused(format!(
            "only {:.0}% of the API's words matched the local clock, under the {:.0}% it takes",
            share * 100.0,
            MIN_RETIME_MATCH * 100.0
        )));
    }
    Ok(words)
}

/// The whole job, from whatever the folder holds. On success the document is also in `result.json`.
pub fn run<E: Ears, A: Transcriber>(
    dir: &Path,
    ears: &E,
    api: Option<ApiPart<'_, A>>,
    language: Option<&str>,
    max_seconds: u64,
    parser: &str,
) -> Result<TimedDocument, WorkerFail> {
    let (mut media, pcm) = decoded(dir, max_seconds)?;
    let spans = spans_of(dir, ears, &pcm)?;
    let clock = heard(dir, ears, &pcm, &spans, language, &mut media)?;
    let clock = clean_words(clock, media.duration_ms);
    let mut usage = TranscribeUsage::default();
    let (words, parser) = match api {
        Some(part) => {
            let words = api_words(dir, &part, &pcm, &spans, &clock, language, &mut usage)?;
            (clean_words(words, media.duration_ms), format!("{parser}+{}+memex-1", part.transcriber.model()))
        }
        None => (clock, format!("{parser}+memex-1")),
    };
    let doc = TimedDocument { parser, media, words, usage };
    write_atomic(dir, RESULT_FILE, &serde_json::to_vec(&doc).map_err(json_error)?)?;
    Ok(doc)
}

#[cfg(test)]
mod tests {
    use super::*;
    use memex_core::testing::FakeTranscriber;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("memex-worker-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    /// One tone per stretch, with silence around each.
    fn recording(dir: &Path, seconds: &[(u32, u32)], total: u32) {
        let mut samples = vec![0i16; (total * 16_000) as usize];
        for (a, b) in seconds {
            for i in *a * 16_000..*b * 16_000 {
                samples[i as usize] = ((i as f32 * 0.1).sin() * 20_000.0) as i16;
            }
        }
        fs::write(dir.join(SOURCE_FILE), crate::decode::wav_for_tests(16_000, 1, &samples)).unwrap();
    }

    struct FakeEars {
        hearing: std::sync::Arc<FakeTranscriber>,
        precise_loads: AtomicU32,
        coarse_loads: AtomicU32,
        vad_runs: AtomicU32,
    }

    impl FakeEars {
        fn new() -> Self {
            Self {
                hearing: Default::default(),
                precise_loads: AtomicU32::new(0),
                coarse_loads: AtomicU32::new(0),
                vad_runs: AtomicU32::new(0),
            }
        }
    }

    struct Shared(std::sync::Arc<FakeTranscriber>);

    impl Transcriber for Shared {
        fn model(&self) -> String {
            self.0.model()
        }

        fn transcribe(&self, pcm: &[f32], language: Option<&str>) -> Result<memex_core::Transcribed, TranscribeError> {
            self.0.transcribe(pcm, language)
        }
    }

    impl Ears for FakeEars {
        type T = Shared;

        fn speech_spans(&self, pcm: &PcmFile) -> Result<Vec<Span>, WorkerFail> {
            self.vad_runs.fetch_add(1, Ordering::SeqCst);
            let all = pcm.read(0, pcm.samples()).map_err(WorkerFail::io)?;
            let (mut out, mut open): (Vec<Span>, Option<u64>) = (vec![], None);
            for (k, second) in all.chunks(16_000).enumerate() {
                let k = k as u64;
                let loud = second.iter().any(|s| s.abs() > 0.01);
                match (loud, open) {
                    (true, None) => open = Some(k),
                    (false, Some(a)) => {
                        out.push(Span {
                            start: a * 16_000,
                            end: k * 16_000,
                        });
                        open = None;
                    }
                    _ => {}
                }
            }
            if let Some(a) = open {
                out.push(Span {
                    start: a * 16_000,
                    end: pcm.samples(),
                });
            }
            Ok(out)
        }

        fn precise(&self) -> Result<Shared, WorkerFail> {
            self.precise_loads.fetch_add(1, Ordering::SeqCst);
            Ok(Shared(self.hearing.clone()))
        }

        fn coarse(&self) -> Result<Shared, WorkerFail> {
            self.coarse_loads.fetch_add(1, Ordering::SeqCst);
            Ok(Shared(self.hearing.clone()))
        }
    }

    const NO_API: Option<ApiPart<'static, FakeTranscriber>> = None;

    #[test]
    fn words_land_on_the_original_timeline() {
        let d = temp("timeline");
        recording(&d, &[(2, 4), (10, 11)], 15);
        let ears = FakeEars::new();
        let doc = run(&d, &ears, NO_API, Some("en"), 3600, "fake").unwrap();
        let starts: Vec<i64> = doc.words.iter().map(|w| w.start_ms).collect();
        assert_eq!(starts, vec![2000, 3000, 10_000]);
        assert_eq!(doc.media.duration_ms, 15_000);
        assert_eq!(doc.parser, "fake+memex-1");
        assert_eq!(doc.usage, TranscribeUsage::default());
        let saved: TimedDocument = serde_json::from_slice(&fs::read(d.join(RESULT_FILE)).unwrap()).unwrap();
        assert_eq!(saved, doc);
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn silence_gives_no_words_and_no_call() {
        let d = temp("silence");
        recording(&d, &[], 5);
        let ears = FakeEars::new();
        let doc = run(&d, &ears, NO_API, None, 3600, "fake").unwrap();
        assert!(doc.words.is_empty());
        assert_eq!(ears.hearing.calls(), 0);
        assert_eq!(ears.precise_loads.load(Ordering::SeqCst), 0, "no model is loaded for nothing");
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn a_new_process_carries_on_from_what_was_saved() {
        let d = temp("resume");
        recording(&d, &[(1, 2), (4, 5), (8, 9)], 10);
        let ears = FakeEars::new();
        let (_, pcm) = decoded(&d, 3600).unwrap();
        let spans = spans_of(&d, &ears, &pcm).unwrap();
        assert_eq!(spans.len(), 3);
        // The first stretch finished before the process died.
        let done = vec![TimedWord {
            text: "saved".into(),
            start_ms: 1000,
            end_ms: 1400,
            speaker: None,
        }];
        let line = |words| Line {
            i: 0,
            words,
            coarse: false,
            skipped: false,
            usage: TranscribeUsage::default(),
        };
        let path = d.join(PROGRESS_FILE);
        append(&path, &line(None)).unwrap();
        append(&path, &line(Some(done))).unwrap();
        let doc = run(&d, &ears, NO_API, None, 3600, "fake").unwrap();
        assert_eq!(doc.words[0].text, "saved");
        assert_eq!(doc.words.iter().map(|w| w.start_ms).collect::<Vec<_>>(), vec![1000, 4000, 8000]);
        assert_eq!(ears.hearing.calls(), 2, "only the two left");
        assert_eq!(ears.vad_runs.load(Ordering::SeqCst), 1, "VAD isn't run again");
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn a_stretch_that_crashed_runs_on_token_times_and_is_counted() {
        let d = temp("crash1");
        recording(&d, &[(1, 2), (4, 5)], 7);
        let ears = FakeEars::new();
        let (_, pcm) = decoded(&d, 3600).unwrap();
        spans_of(&d, &ears, &pcm).unwrap();
        let path = d.join(PROGRESS_FILE);
        let start = Line {
            i: 0,
            words: None,
            coarse: false,
            skipped: false,
            usage: TranscribeUsage::default(),
        };
        append(&path, &start).unwrap();
        let doc = run(&d, &ears, NO_API, None, 3600, "fake").unwrap();
        assert_eq!(doc.media.coarse_spans, 1);
        assert_eq!(doc.words.len(), 2);
        assert_eq!(ears.coarse_loads.load(Ordering::SeqCst), 1);
        assert_eq!(ears.precise_loads.load(Ordering::SeqCst), 1, "the stretch after it goes back to word times");
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn a_stretch_that_crashed_twice_is_skipped_and_counted() {
        let d = temp("crash2");
        recording(&d, &[(1, 2), (4, 5)], 7);
        let ears = FakeEars::new();
        let (_, pcm) = decoded(&d, 3600).unwrap();
        spans_of(&d, &ears, &pcm).unwrap();
        let path = d.join(PROGRESS_FILE);
        let start = || Line {
            i: 0,
            words: None,
            coarse: false,
            skipped: false,
            usage: TranscribeUsage::default(),
        };
        append(&path, &start()).unwrap();
        append(&path, &start()).unwrap();
        let doc = run(&d, &ears, NO_API, None, 3600, "fake").unwrap();
        assert_eq!(doc.media.skipped_spans, 1);
        assert_eq!(doc.words.iter().map(|w| w.start_ms).collect::<Vec<_>>(), vec![4000]);
        // Counted once across a further restart too.
        fs::remove_file(d.join(RESULT_FILE)).unwrap();
        let again = run(&d, &ears, NO_API, None, 3600, "fake").unwrap();
        assert_eq!(again.media.skipped_spans, 1);
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn a_line_cut_short_by_a_crash_counts_as_not_written() {
        let d = temp("cut");
        fs::write(d.join("log"), "{\"i\":0}\n{\"i\":0,\"words\":[{\"te").unwrap();
        let lines = read_lines(&d.join("log")).unwrap();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].words.is_none());
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn a_damaged_file_is_refused_and_nothing_is_heard() {
        let d = temp("damaged");
        fs::write(d.join(SOURCE_FILE), b"not audio").unwrap();
        let ears = FakeEars::new();
        let e = run(&d, &ears, NO_API, None, 3600, "fake").unwrap_err();
        assert!(matches!(e, WorkerFail::Refused(_)), "{e:?}");
        assert_eq!(e.exit_code(), EXIT_REFUSED);
        assert_eq!(ears.vad_runs.load(Ordering::SeqCst), 0);
        fs::remove_dir_all(d).unwrap();
    }

    /// An API that hears the same words as the fake but on a clock of its own.
    struct FakeApi {
        hearing: FakeTranscriber,
        drift_ms: i64,
        mishear: bool,
    }

    impl Transcriber for FakeApi {
        fn model(&self) -> String {
            "fake-api".into()
        }

        fn transcribe(&self, pcm: &[f32], language: Option<&str>) -> Result<memex_core::Transcribed, TranscribeError> {
            let mut t = self.hearing.transcribe(pcm, language)?;
            for (n, w) in t.words.iter_mut().enumerate() {
                w.start_ms += self.drift_ms;
                w.end_ms += self.drift_ms;
                w.speaker = Some("S1".into());
                if self.mishear {
                    w.text = format!("other{n}");
                }
            }
            t.usage = TranscribeUsage {
                input_tokens: 10,
                output_tokens: 5,
                calls: 1,
            };
            Ok(t)
        }
    }

    fn fake_api(drift_ms: i64, mishear: bool) -> FakeApi {
        FakeApi {
            hearing: FakeTranscriber::default(),
            drift_ms,
            mishear,
        }
    }

    #[test]
    fn an_api_that_heard_something_else_is_refused() {
        let d = temp("refuse");
        recording(&d, &[(0, 5)], 6);
        let ears = FakeEars::new();
        let api = fake_api(0, true);
        let e = run(
            &d,
            &ears,
            Some(ApiPart {
                transcriber: &api,
                max_tokens: 1000,
            }),
            None,
            3600,
            "fake",
        )
        .unwrap_err();
        assert!(matches!(&e, WorkerFail::Refused(m) if m.contains("matched")), "{e:?}");
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn an_api_that_agrees_is_kept_on_the_local_clock_with_its_speakers() {
        let d = temp("agree");
        recording(&d, &[(0, 5)], 6);
        let ears = FakeEars::new();
        let api = fake_api(90_000, false);
        let doc = run(
            &d,
            &ears,
            Some(ApiPart {
                transcriber: &api,
                max_tokens: 1000,
            }),
            None,
            3600,
            "fake",
        )
        .unwrap();
        assert_eq!(
            doc.words.iter().map(|w| w.start_ms).collect::<Vec<_>>(),
            vec![0, 1000, 2000, 3000, 4000],
            "the clock's, not the API's"
        );
        assert_eq!(doc.words[0].speaker.as_deref(), Some("0:S1"), "a label names its chunk");
        assert_eq!(doc.usage.input_tokens + doc.usage.output_tokens, 15);
        assert!(doc.parser.contains("fake-api"));
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn a_chunk_with_no_speech_is_never_sent() {
        let d = temp("nosend");
        recording(&d, &[], 5);
        let ears = FakeEars::new();
        let api = fake_api(0, false);
        let doc = run(
            &d,
            &ears,
            Some(ApiPart {
                transcriber: &api,
                max_tokens: 1000,
            }),
            None,
            3600,
            "fake",
        )
        .unwrap();
        assert!(doc.words.is_empty());
        assert_eq!(api.hearing.samples(), 0);
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn spending_stops_at_the_limit_and_an_outage_waits() {
        let d = temp("cap");
        // Over 5.5 minutes, with speech throughout, so several chunks.
        recording(&d, &[(0, 700)], 700);
        let ears = FakeEars::new();
        let api = fake_api(0, false);
        let e = run(
            &d,
            &ears,
            Some(ApiPart {
                transcriber: &api,
                max_tokens: 10,
            }),
            None,
            3600,
            "fake",
        )
        .unwrap_err();
        assert!(matches!(&e, WorkerFail::Refused(m) if m.contains("token limit")), "{e:?}");
        assert_eq!(api.hearing.calls(), 1, "it stopped before the next call");
        api.hearing.fail(true);
        let e = run(
            &d,
            &ears,
            Some(ApiPart {
                transcriber: &api,
                max_tokens: 10_000,
            }),
            None,
            3600,
            "fake",
        )
        .unwrap_err();
        assert!(matches!(e, WorkerFail::Waiting(_)), "{e:?}");
        assert_eq!(e.exit_code(), EXIT_WAITING);
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn with_the_api_off_nothing_but_the_local_model_hears_anything() {
        let d = temp("off");
        recording(&d, &[(0, 2)], 3);
        let ears = FakeEars::new();
        let api = fake_api(0, false);
        // The API exists here and isn't given to the job, as settings wouldn't build it.
        let doc = run(&d, &ears, NO_API, None, 3600, "fake").unwrap();
        assert!(!doc.words.is_empty());
        assert_eq!(api.hearing.calls(), 0);
        assert!(!d.join(API_FILE).exists());
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn cuts_for_the_api_land_in_the_quiet() {
        let d = temp("cuts");
        // Loud everywhere except one quiet second at 290 s.
        let mut samples = vec![];
        for i in 0..16_000u64 * 400 {
            let quiet = (290 * 16_000..291 * 16_000).contains(&i);
            samples.extend_from_slice(&(if quiet { 0.0f32 } else { 0.5f32 }).to_le_bytes());
        }
        fs::write(d.join("pcm"), samples).unwrap();
        let pcm = PcmFile::open(&d.join("pcm")).unwrap();
        let chunks = api_chunks(&pcm).unwrap();
        assert_eq!(chunks.len(), 2);
        let cut = chunks[0].end as f64 / 16_000.0;
        assert!((290.0..291.0).contains(&cut), "{cut}");
        assert_eq!(chunks[1].start, chunks[0].end);
        assert_eq!(chunks[1].end, pcm.samples());
        fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn reading_past_the_end_gives_what_is_there() {
        let d = temp("read");
        fs::write(d.join("pcm"), [1.0f32, 2.0, 3.0].iter().flat_map(|f| f.to_le_bytes()).collect::<Vec<u8>>()).unwrap();
        let pcm = PcmFile::open(&d.join("pcm")).unwrap();
        assert_eq!(pcm.samples(), 3);
        assert_eq!(pcm.read(1, 100).unwrap(), vec![2.0, 3.0]);
        assert!(pcm.read(5, 1).unwrap().is_empty());
        fs::remove_dir_all(d).unwrap();
    }
}
