//! The real transcriber on spec 027's recordings: whisper in the real worker binary, with real decoding, and Postgres for what follows. The agent is scripted and the labels come from a synthesizer, so nothing here is hand-labelled or says anything about a model's choice of quotes. No provider is called: the API's tests talk to a fake on this machine.
//!
//! Needs `MEMEX_TEST_DATABASE_URL`, `MEMEX_TEST_ASR_MODELS` (the model files, flat), `MEMEX_TEST_ASR_CORPUS` (spec 027's `corpus` folder) and, for the damaged files, `MEMEX_TEST_ASR_BAD` (its `out/bad`). See AGENTS.md.
// These tests read the environment, time things and print what they measured.
#![allow(clippy::disallowed_methods, clippy::disallowed_macros)]
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use memex_asr::resample::Resampler;
use memex_asr::worker::{RESULT_FILE, SOURCE_FILE};
use memex_core::meeting::TimedIndex;
use memex_core::testing::{FakeEmbedder, FakeRasterizer, FakeReranker, ManualClock, MemBlobStore, MemRegistry, ScriptedAgent};
use memex_core::{EXTRACT_JOB, ExtractionLimits, SearchEdges, Server, ServerEdges, TRANSCRIBE_JOB, Transcriber, UploadLimits, UploadRequest};
use memex_lexical::TantivyLexical;
use memex_store_postgres::PgStore;
use memex_types::{AgentReply, AgentRequest, AgentUsage, ChangeSetState, JobState, ObjectId, TimedDocument, TimedWord, VaultId};
use memex_vector::PgVectors;
use memexvault::edges::{PgVectorIndex, SystemClock, SystemSeed, TantivyIndex};
use memexvault::pg_edge::PostgresStore;
use memexvault::transcribe_job::{TranscribeCommand, TranscribeHandler};
use memexvault::workers::run_one;
use serde_json::{Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};
use tower::ServiceExt;

const BIN: &str = env!("CARGO_BIN_EXE_memexvault");

fn env_path(name: &str) -> PathBuf {
    PathBuf::from(std::env::var(name).unwrap_or_else(|_| panic!("not run: {name} isn't set")))
}

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("memex-eval-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&d);
    fs::create_dir_all(&d).unwrap();
    d
}

/// The models in the layout `models fetch` makes, without copying 574 MB.
fn models_dir() -> PathBuf {
    let flat = env_path("MEMEX_TEST_ASR_MODELS");
    let dir = std::env::temp_dir().join(format!("memex-eval-models-{}", std::process::id()));
    let whisper = dir.join("whisper");
    fs::create_dir_all(&whisper).unwrap();
    for file in ["ggml-large-v3-turbo-q5_0.bin", "ggml-silero-v6.2.0.bin"] {
        let link = whisper.join(file);
        if !link.exists() {
            std::os::unix::fs::symlink(flat.join(file), link).unwrap();
        }
    }
    dir
}

fn corpus(name: &str) -> PathBuf {
    env_path("MEMEX_TEST_ASR_CORPUS").join("synth").join(name)
}

struct Truth {
    words: Vec<TimedWord>,
}

fn truth_of(stem: &str) -> Truth {
    let v: Value = serde_json::from_slice(&fs::read(corpus(&format!("{stem}.json"))).unwrap()).unwrap();
    let words = v["words"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| TimedWord {
            text: w["w"].as_str().unwrap().to_string(),
            start_ms: w["s"].as_i64().unwrap(),
            end_ms: w["e"].as_i64().unwrap(),
            speaker: None,
        })
        .collect();
    Truth { words }
}

fn norm(w: &str) -> String {
    w.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// For each word of `heard`, the index of the truth word it aligned to, by a diff on lowercased alphanumerics.
fn align(heard: &[TimedWord], truth: &[TimedWord]) -> Vec<Option<usize>> {
    let a: Vec<String> = heard.iter().map(|w| norm(&w.text)).collect();
    let b: Vec<String> = truth.iter().map(|w| norm(&w.text)).collect();
    let mut out = vec![None; heard.len()];
    for op in similar::capture_diff_slices(similar::Algorithm::Myers, &a, &b) {
        if let similar::DiffOp::Equal { old_index, new_index, len } = op {
            for k in 0..len {
                out[old_index + k] = Some(new_index + k);
            }
        }
    }
    out
}

fn median(mut v: Vec<i64>) -> i64 {
    v.sort_unstable();
    v.get(v.len() / 2).copied().unwrap_or(-1)
}

/// Runs the worker binary directly under `/usr/bin/time -l`, for its peak memory. Returns the exit code, peak RSS in MB and seconds.
fn run_worker(dir: &Path, language: Option<&str>, envs: &[(&str, &str)]) -> (Option<i32>, u64, f64, String) {
    let models = models_dir();
    let mut c = Command::new("/usr/bin/time");
    c.arg("-l").arg(BIN).arg("transcribe-worker").arg(dir);
    if let Some(l) = language {
        c.arg(l);
    }
    c.env("MEMEX_MODELS_DIR", &models)
        .env_remove("MEMEX_DATABASE_URL")
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    for (k, v) in envs {
        c.env(k, v);
    }
    let started = Instant::now();
    let out = c.output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let rss = stderr
        .lines()
        .find(|l| l.contains("maximum resident set size"))
        .and_then(|l| l.split_whitespace().next())
        .and_then(|n| n.parse::<u64>().ok())
        .unwrap_or(0)
        / (1024 * 1024);
    // `time` passes a signal death on as 128 and more or as nothing, depending on the system.
    (out.status.code(), rss, started.elapsed().as_secs_f64(), stderr)
}

fn result_of(dir: &Path) -> TimedDocument {
    serde_json::from_slice(&fs::read(dir.join(RESULT_FILE)).unwrap()).unwrap()
}

struct Real;

impl ServerEdges for Real {
    type Registry = MemRegistry;
    type Store = PostgresStore;
    type Blobs = MemBlobStore<ManualClock>;
    type Clock = SystemClock;
    type Seed = SystemSeed;
    type Rasterizer = FakeRasterizer;
    type Lexical = TantivyIndex;
    type Vector = PgVectorIndex;
    type Embedder = FakeEmbedder;
    type Reranker = FakeReranker;
    type Caches = memex_core::NoCaches;
}

struct Rig {
    s: Arc<Server<Real>>,
    token: String,
    admin: memex_core::Caller,
    v: VaultId,
    dir: PathBuf,
}

impl Drop for Rig {
    fn drop(&mut self) {
        let _ = self.s.delete_vault(&self.admin, &self.v);
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn rig(name: &str) -> Rig {
    let url = std::env::var("MEMEX_TEST_DATABASE_URL").expect("not run: MEMEX_TEST_DATABASE_URL isn't set");
    let pg = PgStore::connect(&url, 8).expect("connect");
    pg.migrate().expect("migrate");
    let pg = Arc::new(pg);
    let dir = scratch(name);
    let s = Arc::new(Server::<Real>::new(
        MemRegistry::new(),
        PostgresStore(pg.clone()),
        MemBlobStore::new(ManualClock::starting_at(0)),
        SystemClock::default(),
        SystemSeed,
        SearchEdges {
            lexical: TantivyIndex(TantivyLexical::new(&dir)),
            vector: PgVectorIndex(PgVectors::new(pg.clone())),
            embedder: FakeEmbedder::default(),
            reranker: FakeReranker::default(),
        },
    ));
    s.set_extraction(Some(ExtractionLimits {
        batch_tokens: 400,
        document_budget: u64::MAX / 4,
        ..ExtractionLimits::default()
    }));
    let token = s.bootstrap("root").unwrap().token;
    let admin = s.authenticate(&token).unwrap();
    let v = s.create_vault(&admin, name).unwrap().id;
    Rig { s, token, admin, v, dir }
}

impl Rig {
    fn handler(&self) -> TranscribeHandler<Real> {
        TranscribeHandler {
            server: self.s.clone(),
            command: TranscribeCommand {
                program: BIN.into(),
                args: vec!["transcribe-worker".into()],
                envs: vec![("MEMEX_MODELS_DIR".into(), models_dir().to_string_lossy().into_owned())],
                env_remove: vec!["MEMEX_DATABASE_URL".into(), "MEMEX_TRANSCRIBE_API_KEY".into()],
                max_rss_mb: 8192,
                stall: Duration::from_secs(1800),
            },
            root: self.dir.join("vaults"),
            stop: Arc::new(AtomicBool::new(false)),
        }
    }

    fn upload(&self, name: &str, bytes: &[u8], language: Option<&str>) -> Result<ObjectId, memex_core::ServerError> {
        let req = UploadRequest {
            name: name.into(),
            trust_tier: "unverified".into(),
            metadata: language.map_or_else(|| json!({}), |l| json!({"language": l})),
            replaces: None,
        };
        let mut b = bytes;
        Ok(self.s.upload(&self.admin, &self.v, req, UploadLimits::default(), &mut b)?.source.header.id)
    }

    /// Runs the job queue's next transcription through the pool's own code path.
    fn transcribe_next(&self) -> f64 {
        let started = Instant::now();
        assert!(run_one(self.s.as_ref(), &self.handler(), TRANSCRIBE_JOB, "eval"), "a job should be waiting");
        started.elapsed().as_secs_f64()
    }

    fn extract(&self) {
        let agent = ScriptedAgent::answering(every_sentence);
        while let Some(job) = self.s.claim_job(&self.v, EXTRACT_JOB, "w").unwrap() {
            let rows = self.s.run_extraction_job(&self.v, &job, &agent).unwrap();
            assert!(self.s.complete_job(&self.v, &job, rows).unwrap());
        }
    }

    fn approve_all(&self) -> usize {
        let mut total = 0;
        loop {
            let pending = self.s.list_changesets(&self.admin, &self.v, Some(ChangeSetState::Pending), None, 200).unwrap();
            if pending.is_empty() {
                return total;
            }
            for c in &pending {
                self.s.approve(&self.admin, &self.v, &c.header.id, None).unwrap();
            }
            total += pending.len();
        }
    }

    fn get(&self, rt: &tokio::runtime::Runtime, path: &str, range: Option<&str>) -> (StatusCode, Vec<u8>) {
        let mut req = Request::builder().uri(path).header("authorization", format!("Bearer {}", self.token));
        if let Some(r) = range {
            req = req.header("range", r);
        }
        rt.block_on(async {
            let res = memex_http::router(self.s.clone()).oneshot(req.body(Body::empty()).unwrap()).await.unwrap();
            let status = res.status();
            (status, to_bytes(res.into_body(), 1 << 30).await.unwrap().to_vec())
        })
    }

    fn job_state(&self, kind: &str) -> Vec<(JobState, Option<String>)> {
        self.s
            .list_jobs(&self.admin, &self.v, Some(kind), None, None, 1000)
            .unwrap()
            .into_iter()
            .map(|j| (j.state, j.attempts.last().and_then(|a| a.error.clone())))
            .collect()
    }
}

fn segments_of(req: &AgentRequest) -> Vec<(String, String)> {
    let doc: Value = serde_json::from_str(req.input.split("\n\nYour previous").next().unwrap()).unwrap();
    doc["segments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| (s["id"].as_str().unwrap().to_string(), s["text"].as_str().unwrap().to_string()))
        .collect()
}

/// One claim per sentence of what it's given, with none left out, keeping those of at least 3 words. This is item 1's rule, in `spec.md`.
fn every_sentence(req: &AgentRequest) -> Result<AgentReply, memex_types::AgentError> {
    let mut claims = vec![];
    for (id, text) in segments_of(req) {
        for sentence in text.split_inclusive(['.', '!', '?']) {
            let quote = sentence.split_whitespace().collect::<Vec<_>>().join(" ");
            if quote.split_whitespace().count() >= 3 {
                claims.push(json!({"segment_id": id, "quote": quote, "subject": format!("{id} {quote}"), "predicate": "sägs", "value": "i mötet"}));
            }
        }
    }
    Ok(AgentReply {
        json: json!({"claims": claims, "entities": {}}).to_string(),
        usage: AgentUsage {
            input_tokens: 1,
            output_tokens: 1,
            thinking_tokens: 0,
        },
        tool_calls: 0,
    })
}

/// What a WAV's header says: where its samples start, its rate, and its sample width in bytes.
fn wav_layout(head: &[u8]) -> (u64, u32, u32) {
    let (mut at, mut rate, mut width) = (12usize, 0, 2);
    while at + 8 <= head.len() {
        let size = u32::from_le_bytes(head[at + 4..at + 8].try_into().unwrap()) as usize;
        if &head[at..at + 4] == b"fmt " {
            rate = u32::from_le_bytes(head[at + 12..at + 16].try_into().unwrap());
            width = u32::from(u16::from_le_bytes(head[at + 22..at + 24].try_into().unwrap())) / 8;
        }
        if &head[at..at + 4] == b"data" {
            return (at as u64 + 8, rate, width);
        }
        at += 8 + size + (size & 1);
    }
    panic!("no data chunk in the first bytes");
}

/// Items 1, 2, 5 and 8 on one recording, end to end: upload, the job with real whisper, a scripted agent's claims approved by a person, and the provenance API's answers.
fn end_to_end(stem: &str, language: &str) {
    let name = format!("{stem}.wav");
    let bytes = fs::read(corpus(&name)).unwrap();
    let r = rig(&format!("e2e-{}", stem.replace('_', "-")));
    let rt = tokio::runtime::Builder::new_multi_thread().enable_all().worker_threads(2).build().unwrap();
    let source = r.upload(&name, &bytes, Some(language)).unwrap();
    let seconds = r.transcribe_next();
    let jobs = r.job_state(TRANSCRIBE_JOB);
    assert_eq!(jobs[0].0, JobState::Done, "{jobs:?}");
    r.extract();
    let approved = r.approve_all();

    // The word table the API serves, and the truth from the synthesizer's markers.
    let renditions = r.s.list_renditions(&r.admin, &r.v, &source).unwrap();
    let current = renditions.iter().find(|x| x.superseded_by.is_none()).unwrap();
    let table = r.s.rendition_blob(&r.admin, &r.v, &current.header.id, true).unwrap();
    let doc: TimedDocument = serde_json::from_slice(&table).unwrap();
    let duration = doc.media.duration_ms;
    let words = doc.words.len();
    let index = TimedIndex::new(doc);
    let truth = truth_of(stem);
    let aligned = align(&index.doc.words, &truth.words);
    let aligned_share = aligned.iter().flatten().count() as f64 / aligned.len().max(1) as f64;
    let starts: Vec<i64> = aligned
        .iter()
        .enumerate()
        .filter_map(|(i, t)| t.map(|t| (index.doc.words[i].start_ms - truth.words[t].start_ms).abs()))
        .collect();
    println!(
        "{stem}: {} s of audio in {seconds:.0} s on a busy machine, {words} words, {:.1}% aligned to truth, median word start error {} ms, {} segments",
        duration / 1000,
        aligned_share * 100.0,
        median(starts),
        current.segment_count
    );

    let citing = r.s.claims_citing_source(&r.admin, &r.v, &source).unwrap();
    let (mut passed, mut no_truth, mut late_start, mut early_end, mut far_start, mut no_anchor) = (0usize, 0usize, 0usize, 0usize, 0usize, 0usize);
    let mut full_alignment = 0usize;
    let (mut lead, mut disagreements) = (vec![], 0usize);
    let mut failures = vec![];
    let mut samples = vec![];
    for c in &citing {
        let lineage = r.s.lineage(&r.admin, &r.v, &c.assertion.header.id).unwrap();
        let e = &lineage.evidence[0];
        // Item 2: every committed claim resolves to a time.
        let Some(anchor) = e.anchor.as_ref().filter(|_| !e.broken) else {
            no_anchor += 1;
            continue;
        };
        let Some(time) = anchor.time.clone() else {
            no_anchor += 1;
            continue;
        };
        let range = memex_core::meeting::words_in(&index.canon.spans, anchor.start as usize, anchor.end as usize);
        // Item 8: the full transcript's range for these same bytes is what provenance gave.
        let path = format!(
            "/api/v1/vaults/{}/sources/{}/transcript?from_ms={}&to_ms={}&words=true&limit=200",
            r.v.0,
            source.0,
            index.doc.words[range.start].end_ms,
            index.doc.words[range.end - 1].start_ms
        );
        let (status, body) = r.get(&rt, &path, None);
        assert_eq!(status, StatusCode::OK);
        let t: Value = serde_json::from_slice(&body).unwrap();
        let parts = t["parts"].as_array().unwrap();
        let (t0, t1) = (parts[0]["t_start_ms"].as_i64(), parts.last().unwrap()["t_end_ms"].as_i64());
        if t0 != Some(time.t_start_ms) || t1 != Some(time.t_end_ms) {
            disagreements += 1;
        }
        // Item 1's rule.
        let hits: Vec<usize> = aligned[range.clone()].iter().flatten().copied().collect();
        if hits.len() == range.len() {
            full_alignment += 1;
        }
        if hits.len() * 2 < range.len() || hits.is_empty() {
            no_truth += 1;
            failures.push(format!("no truth: {}", e.evidence.quote));
            continue;
        }
        let (truth_start, truth_end) = (truth.words[hits[0]].start_ms, truth.words[*hits.last().unwrap()].end_ms);
        let contains_start = time.t_start_ms <= truth_start;
        let contains_end = time.t_end_ms >= truth_end;
        let near = (time.t_start_ms - truth_start).abs() <= 1000;
        lead.push(truth_start - time.t_start_ms);
        if contains_start && contains_end && near {
            passed += 1;
            if samples.len() < 10 && e.evidence.quote.split_whitespace().count() >= 8 {
                samples.push((e.evidence.quote.clone(), time.t_start_ms, time.t_end_ms));
            }
        } else {
            late_start += usize::from(!contains_start);
            early_end += usize::from(!contains_end);
            far_start += usize::from(!near);
            failures.push(format!(
                "{} to {} against truth {truth_start} to {truth_end}: {}",
                time.t_start_ms, time.t_end_ms, e.evidence.quote
            ));
        }
    }
    let n = citing.len();
    let share = passed as f64 / n.max(1) as f64 * 100.0;
    println!(
        "{stem} item 1: {n} claims from {approved} approved proposals, {passed} pass ({share:.1}%). Failures: {late_start} start after the truth, {early_end} end before it, {far_start} start over 1 s away, {no_truth} with under half their words aligned. {full_alignment} had every word aligned. Median lead before the truth start {} ms",
        median(lead)
    );
    println!("{stem} item 2: {no_anchor} of {n} claims without a resolving time anchor");
    println!("{stem} item 8: {disagreements} of {n} ranges differ between the transcript and provenance");
    for f in failures.iter().take(12) {
        println!("  {f}");
    }

    // Item 5: what a player does. It reads the header by Range, works out the byte for the cited time, and asks for just that. The bytes are the original's, and a transcriber hears the quote in them.
    let stream = format!("/api/v1/vaults/{}/sources/{}/stream", r.v.0, source.0);
    let (status, head) = r.get(&rt, &stream, Some("bytes=0-4095"));
    assert_eq!(status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(&head[..], &bytes[..4096]);
    let (data, rate, width) = wav_layout(&head);
    let models = models_dir();
    let ear = memex_asr::WhisperLocal::new(&models.join(memex_asr::whisper::WHISPER_FILE), 4, true).unwrap();
    let (mut played, mut heard_ok, mut heard_share) = (0usize, 0usize, 0.0f64);
    for (quote, t0, t1) in &samples {
        let frame = u64::from(width);
        let a = data + (*t0 as u64) * u64::from(rate) / 1000 * frame;
        let b = data + (*t1 as u64) * u64::from(rate) / 1000 * frame - 1;
        let (status, part) = r.get(&rt, &stream, Some(&format!("bytes={a}-{b}")));
        assert_eq!(status, StatusCode::PARTIAL_CONTENT);
        assert_eq!(&part[..], &bytes[a as usize..=b as usize], "the original's own bytes");
        let pcm: Vec<f32> = part.as_chunks::<2>().0.iter().map(|s| f32::from(i16::from_le_bytes(*s)) / 32768.0).collect();
        let mut resampler = Resampler::new(rate, 16_000);
        let mut out = vec![];
        resampler.push(&pcm, &mut out);
        resampler.finish(&mut out);
        let heard = ear.transcribe(&out, Some(language)).unwrap();
        let heard: Vec<String> = heard.words.iter().map(|w| norm(&w.text)).collect();
        let wanted: Vec<String> = quote.split_whitespace().map(norm).collect();
        let found = wanted.iter().filter(|w| heard.contains(w)).count() as f64 / wanted.len() as f64;
        played += 1;
        heard_share += found;
        heard_ok += usize::from(found >= 0.8);
    }
    println!(
        "{stem} item 5: {played} cited ranges fetched by Range and byte-identical to the original. A fresh transcriber heard at least 80% of the quote's words in {heard_ok} of them, {:.0}% on average",
        heard_share / played.max(1) as f64 * 100.0
    );

    assert_eq!(no_anchor, 0, "item 2");
    assert_eq!(disagreements, 0, "item 8");
    assert_eq!(played, samples.len());
    assert!(played >= 5, "item 5 had too few long quotes to play");
    if stem == std::env::var("MEMEX_ASR_DECIDES").as_deref().unwrap_or("en_60m") {
        assert!(n >= 50, "item 1: {n} claims is a shortfall against 50");
        assert!(share >= 95.0, "item 1 fails its rule: {share:.1}%");
    }
}

#[test]
#[ignore = "needs Postgres, the models and spec 027's corpus"]
fn asr_item_1_on_english() {
    end_to_end("en_5m", "en");
}

#[test]
#[ignore = "needs Postgres, the models and spec 027's corpus"]
fn asr_item_1_on_the_other_languages() {
    for (stem, language) in [("sv_5m", "sv"), ("de_5m", "de"), ("es_5m", "es")] {
        end_to_end(stem, language);
    }
}

#[test]
#[ignore = "needs Postgres, the models and spec 027's corpus, and takes minutes"]
fn asr_item_1_on_an_hour() {
    end_to_end("en_60m", "en");
}

/// Item 7: what goes wrong fails one job. Each is a job through the pool's own code, and a good recording finishes after them all.
#[test]
#[ignore = "needs Postgres, the models and spec 027's damaged files"]
fn asr_bad_files_fail_one_job_each() {
    let bad = env_path("MEMEX_TEST_ASR_BAD");
    let r = rig("bad-files");
    let mut names: Vec<String> = fs::read_dir(&bad)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    let (mut done, mut dead, mut refused) = (0, 0, 0);
    for name in names {
        let bytes = fs::read(bad.join(&name)).unwrap();
        match r.upload(&name, &bytes, Some("en")) {
            Err(e) => {
                refused += 1;
                println!("{name}: refused at upload: {e}");
            }
            Ok(source) => {
                let before = r.job_state(TRANSCRIBE_JOB).len();
                let media = r.s.get_source(&r.admin, &r.v, &source).unwrap().media_type;
                if !media.starts_with("audio/") && !media.starts_with("video/") {
                    refused += 1;
                    println!("{name}: taken as {media} and not as a recording");
                    continue;
                }
                let seconds = r.transcribe_next();
                let jobs = r.s.list_jobs(&r.admin, &r.v, Some(TRANSCRIBE_JOB), None, None, 1000).unwrap();
                assert_eq!(jobs.len(), before);
                let job = jobs.iter().find(|j| j.input["source_id"] == source.0).unwrap();
                let renditions = r.s.list_renditions(&r.admin, &r.v, &source).unwrap();
                let words = renditions.first().map_or(0, |x| x.segment_count);
                match job.state {
                    JobState::Done => done += 1,
                    JobState::Dead => dead += 1,
                    other => panic!("{name}: {other:?}, {:?}", job.attempts),
                }
                println!(
                    "{name}: {:?} in {seconds:.1} s, {words} segments, {}",
                    job.state,
                    job.attempts.last().and_then(|a| a.error.as_deref()).unwrap_or("no error")
                );
            }
        }
    }
    println!("bad files: {done} done, {dead} dead with a reason, {refused} refused at upload");
    assert!(dead >= 4, "the damaged ones fail");
    // Nothing stayed behind, and the server still works.
    let good = r
        .upload("good.wav", &fs::read(corpus("en_5m.wav")).unwrap()[..44 + 22_050 * 2 * 30], Some("en"))
        .unwrap();
    r.transcribe_next();
    let renditions = r.s.list_renditions(&r.admin, &r.v, &good).unwrap();
    assert_eq!(renditions.len(), 1);
    assert!(renditions[0].segment_count >= 1);
}

fn wav_header(rate: u32, bytes: u64) -> Vec<u8> {
    let data = u32::try_from(bytes).expect("under 4 GB");
    let mut o = b"RIFF".to_vec();
    o.extend_from_slice(&(36 + data).to_le_bytes());
    o.extend_from_slice(b"WAVEfmt ");
    o.extend_from_slice(&16u32.to_le_bytes());
    o.extend_from_slice(&1u16.to_le_bytes());
    o.extend_from_slice(&1u16.to_le_bytes());
    o.extend_from_slice(&rate.to_le_bytes());
    o.extend_from_slice(&(rate * 2).to_le_bytes());
    o.extend_from_slice(&2u16.to_le_bytes());
    o.extend_from_slice(&16u16.to_le_bytes());
    o.extend_from_slice(b"data");
    o.extend_from_slice(&data.to_le_bytes());
    o
}

/// Silence of this length as a sparse file, so it takes no room on disk until decoded.
fn silent_wav(path: &Path, seconds: u64) {
    use std::io::Write;
    let bytes = seconds * 16_000 * 2;
    let mut f = fs::File::create(path).unwrap();
    f.write_all(&wav_header(16_000, bytes)).unwrap();
    f.set_len(44 + bytes).unwrap();
}

/// Item 7: silence, no speech, and very long files, each with the worker's peak memory.
#[test]
#[ignore = "needs the models, and takes minutes and a few GB of disk"]
fn asr_long_quiet_and_noisy_stay_within_memory() {
    let limit_mb = 4096;
    // Over the 6-hour limit: decoding stops at the limit, and it's refused.
    let d = scratch("over");
    silent_wav(&d.join(SOURCE_FILE), 7 * 3600);
    let (code, rss, s, stderr) = run_worker(&d, None, &[]);
    println!(
        "7 h of silence: exit {code:?}, peak {rss} MB, {s:.0} s on a busy machine: {}",
        stderr.lines().next().unwrap_or("")
    );
    assert_eq!(code, Some(3));
    assert!(stderr.contains("limit"), "{stderr}");
    assert!(rss < limit_mb);
    fs::remove_dir_all(d).unwrap();

    // Under it: a long silence reads through VAD, which is all it needs, and gives no words.
    let d = scratch("quiet");
    silent_wav(&d.join(SOURCE_FILE), 4 * 3600);
    let (code, rss, s, _) = run_worker(&d, None, &[]);
    let doc = result_of(&d);
    println!(
        "4 h of silence: exit {code:?}, peak {rss} MB, {s:.0} s on a busy machine, {} words",
        doc.words.len()
    );
    assert_eq!(code, Some(0));
    assert!(doc.words.is_empty());
    assert!(rss < limit_mb);
    fs::remove_dir_all(d).unwrap();

    // No speech: a minute of noise. Whatever whisper makes of it, the job ends and memory holds.
    let d = scratch("noise");
    let mut noise = wav_header(16_000, 16_000 * 2 * 60);
    let mut x = 88172645463325252u64;
    for _ in 0..16_000 * 60 {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        noise.extend_from_slice(&((x % 8000) as i16 - 4000).to_le_bytes());
    }
    fs::write(d.join(SOURCE_FILE), noise).unwrap();
    let (code, rss, s, _) = run_worker(&d, None, &[]);
    let doc = result_of(&d);
    println!(
        "1 min of noise: exit {code:?}, peak {rss} MB, {s:.0} s on a busy machine, {} words",
        doc.words.len()
    );
    assert_eq!(code, Some(0));
    assert!(rss < limit_mb);
    fs::remove_dir_all(d).unwrap();

    // Real speech for an hour, the longest in the corpus.
    let d = scratch("hour");
    fs::copy(corpus("en_60m.wav"), d.join(SOURCE_FILE)).unwrap();
    let (code, rss, s, _) = run_worker(&d, Some("en"), &[]);
    let doc = result_of(&d);
    let truth = truth_of("en_60m");
    let aligned = align(&doc.words, &truth.words);
    let mut last_drift = 0;
    for (i, t) in aligned.iter().enumerate().rev() {
        if let Some(t) = t {
            last_drift = doc.words[i].start_ms - truth.words[*t].start_ms;
            break;
        }
    }
    println!(
        "60 min of speech: exit {code:?}, peak {rss} MB, {s:.0} s on a busy machine ({:.1}× real time), {} words, last word off by {last_drift} ms, coarse {}, skipped {}",
        3600.0 / s,
        doc.words.len(),
        doc.media.coarse_spans,
        doc.media.skipped_spans
    );
    assert_eq!(code, Some(0));
    assert!(rss < limit_mb);
    assert!(last_drift.abs() < 1000);
    fs::remove_dir_all(d).unwrap();
}

/// Killing the worker in the middle costs what it was doing, as spec 027 found whisper.cpp can.
#[test]
#[ignore = "needs the models and spec 027's corpus"]
fn asr_a_killed_worker_carries_on_from_its_progress() {
    let d = scratch("killed");
    fs::copy(corpus("en_5m.wav"), d.join(SOURCE_FILE)).unwrap();
    let models = models_dir();
    let mut child = Command::new(BIN)
        .arg("transcribe-worker")
        .arg(&d)
        .arg("en")
        .env("MEMEX_MODELS_DIR", &models)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let done_lines = |d: &Path| {
        fs::read_to_string(d.join("progress.jsonl"))
            .unwrap_or_default()
            .lines()
            .filter(|l| l.contains("\"words\""))
            .count()
    };
    let started = Instant::now();
    while done_lines(&d) < 3 {
        assert!(started.elapsed() < Duration::from_secs(1200), "no progress");
        assert!(child.try_wait().unwrap().is_none(), "it finished too soon to kill");
        std::thread::sleep(Duration::from_millis(50));
    }
    child.kill().unwrap();
    child.wait().unwrap();
    let saved = done_lines(&d);
    let (code, rss, s, _) = run_worker(&d, Some("en"), &[]);
    assert_eq!(code, Some(0));
    let doc = result_of(&d);
    let truth = truth_of("en_5m");
    let aligned = align(&doc.words, &truth.words);
    let errors: Vec<i64> = aligned
        .iter()
        .enumerate()
        .filter_map(|(i, t)| t.map(|t| (doc.words[i].start_ms - truth.words[t].start_ms).abs()))
        .collect();
    println!(
        "killed after {saved} stretches, and carried on: peak {rss} MB, {s:.0} s, {} words, {} coarse, median start error {} ms",
        doc.words.len(),
        doc.media.coarse_spans,
        median(errors)
    );
    assert!(doc.words.len() as f64 > truth.words.len() as f64 * 0.8);
    assert!(doc.media.coarse_spans <= 1);
    fs::remove_dir_all(d).unwrap();
}

/// A fake of the provider, on this machine. It counts what reaches it, and answers with the truth's words on a clock that's off by 30 s, as Gemini's own times are.
struct FakeProvider {
    calls: Arc<AtomicU64>,
    bytes: Arc<AtomicU64>,
    url: String,
    stop: Arc<AtomicBool>,
}

impl FakeProvider {
    fn start(words: Vec<TimedWord>) -> Self {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (calls, bytes, stop) = (Arc::new(AtomicU64::new(0)), Arc::new(AtomicU64::new(0)), Arc::new(AtomicBool::new(false)));
        let (c, b, s) = (calls.clone(), bytes.clone(), stop.clone());
        std::thread::spawn(move || {
            while !s.load(Ordering::Relaxed) {
                let Ok((mut conn, _)) = listener.accept() else {
                    std::thread::sleep(Duration::from_millis(10));
                    continue;
                };
                conn.set_nonblocking(false).unwrap();
                conn.set_read_timeout(Some(Duration::from_secs(30))).unwrap();
                let mut got = vec![];
                let mut buffer = [0u8; 65536];
                let total = loop {
                    let n = conn.read(&mut buffer).unwrap_or(0);
                    if n == 0 {
                        break None;
                    }
                    got.extend_from_slice(&buffer[..n]);
                    let Some(end) = got.windows(4).position(|w| w == b"\r\n\r\n") else { continue };
                    let head = String::from_utf8_lossy(&got[..end]).to_lowercase();
                    let length: usize = head
                        .lines()
                        .find_map(|l| l.strip_prefix("content-length:"))
                        .and_then(|v| v.trim().parse().ok())
                        .unwrap_or(0);
                    if got.len() >= end + 4 + length {
                        break Some(end + 4 + length);
                    }
                };
                c.fetch_add(1, Ordering::SeqCst);
                b.fetch_add(total.unwrap_or(0) as u64, Ordering::SeqCst);
                let reply: Vec<Value> = words
                    .iter()
                    .map(|w| json!({"w": w.text, "s": (w.start_ms + 30_000) as f64 / 1000.0, "e": (w.end_ms + 30_000) as f64 / 1000.0, "spk": "S1"}))
                    .collect();
                let body = json!({"candidates": [{"content": {"parts": [{"text": serde_json::to_string(&reply).unwrap()}]}, "finishReason": "STOP"}], "usageMetadata": {"promptTokenCount": 100, "candidatesTokenCount": 50}}).to_string();
                let _ = write!(
                    conn,
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        Self { calls, bytes, url, stop }
    }
}

impl Drop for FakeProvider {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Item 6, with the real worker binary and real whisper. The fake provider never sees a request unless the API is named. Then its words land on the local clock, off by far less than its own 30 s.
#[test]
#[ignore = "needs the models and spec 027's corpus"]
fn asr_the_api_is_off_unless_named_and_then_runs_on_the_local_clock() {
    let truth = truth_of("en_5m");
    let provider = FakeProvider::start(truth.words.clone());
    let source = fs::read(corpus("en_5m.wav")).unwrap();

    // Off, in each way it can be off: the key, model and address all set, and only the name missing.
    let off = scratch("api-off");
    fs::write(off.join(SOURCE_FILE), &source).unwrap();
    let envs = [
        ("MEMEX_TRANSCRIBE_API_KEY", "fake-key"),
        ("MEMEX_TRANSCRIBE_API_MODEL", "fake-model"),
        ("MEMEX_TRANSCRIBE_API_BASE_URL", provider.url.as_str()),
    ];
    let (code, _, _, stderr) = run_worker(&off, Some("en"), &envs);
    assert_eq!(code, Some(0), "{stderr}");
    let local = result_of(&off);
    println!(
        "API off, with its key, model and address all set: {} requests, {} bytes reached the provider, parser {}",
        provider.calls.load(Ordering::SeqCst),
        provider.bytes.load(Ordering::SeqCst),
        local.parser
    );
    assert_eq!(provider.calls.load(Ordering::SeqCst), 0, "item 6");
    assert_eq!(provider.bytes.load(Ordering::SeqCst), 0, "item 6");
    assert!(!local.parser.contains("gemini"));
    assert!(!off.join("api.jsonl").exists());
    fs::remove_dir_all(off).unwrap();

    // Named. The fake's words carry a clock 30 s off.
    let on = scratch("api-on");
    fs::write(on.join(SOURCE_FILE), &source).unwrap();
    let envs = [
        ("MEMEX_BACKEND_TRANSCRIBER", "gemini"),
        ("MEMEX_TRANSCRIBE_API_KEY", "fake-key"),
        ("MEMEX_TRANSCRIBE_API_MODEL", "fake-model"),
        ("MEMEX_TRANSCRIBE_API_BASE_URL", provider.url.as_str()),
    ];
    let (code, _, _, stderr) = run_worker(&on, Some("en"), &envs);
    assert_eq!(code, Some(0), "{stderr}");
    let doc = result_of(&on);
    let aligned = align(&doc.words, &truth.words);
    let errors: Vec<i64> = aligned
        .iter()
        .enumerate()
        .filter_map(|(i, t)| t.map(|t| (doc.words[i].start_ms - truth.words[t].start_ms).abs()))
        .collect();
    let error = median(errors);
    println!(
        "API named, against a fake: {} requests, {} bytes, parser {}, {} words, median start error {error} ms against the fake's own 30,000 ms, speaker {:?}. A fake says nothing about the provider",
        provider.calls.load(Ordering::SeqCst),
        provider.bytes.load(Ordering::SeqCst),
        doc.parser,
        doc.words.len(),
        doc.words[0].speaker
    );
    assert!(provider.calls.load(Ordering::SeqCst) >= 1);
    assert!(doc.parser.contains("gemini:fake-model"));
    assert!(error < 300, "{error}");
    assert_eq!(doc.words[0].speaker.as_deref(), Some("0:S1"));
    assert_eq!(doc.usage.input_tokens, 100 * provider.calls.load(Ordering::SeqCst));
    fs::remove_dir_all(on).unwrap();
}

/// Each format in spec 027's corpus, with the audio's lateness kept. This checks the decoder's placement on the timeline, against truth.
#[test]
#[ignore = "needs the models and spec 027's corpus"]
fn asr_every_format_stays_on_the_timeline() {
    let truth = truth_of("en_5m");
    let cases: [(&str, i64); 9] = [
        ("en_5m.wav", 0),
        ("en_5m.mp3", 0),
        ("en_5m.m4a", 0),
        ("en_5m.opus", 0),
        ("en_5m.webm", 0),
        ("en_5m.mp4", 0),
        ("en_edge_webm_late.webm", 1500),
        ("en_edge_m4a_late.m4a", 1500),
        ("en_edge_mp4_late.mp4", 1500),
    ];
    let mut worst = 0;
    for (name, shift) in cases {
        let d = scratch("format");
        let bytes = fs::read(corpus(name)).unwrap();
        assert!(
            memex_core::sniff_media_type(&bytes[..4096], name).is_some_and(|m| m.starts_with("audio/") || m.starts_with("video/")),
            "{name}"
        );
        fs::write(d.join(SOURCE_FILE), bytes).unwrap();
        let (code, rss, s, stderr) = run_worker(&d, Some("en"), &[]);
        assert_eq!(code, Some(0), "{name}: {stderr}");
        let doc = result_of(&d);
        let aligned = align(&doc.words, &truth.words);
        let errors: Vec<i64> = aligned
            .iter()
            .enumerate()
            .filter_map(|(i, t)| t.map(|t| (doc.words[i].start_ms - truth.words[t].start_ms - shift).abs()))
            .collect();
        let error = median(errors);
        println!(
            "{name}: {} words, median start error {error} ms, {} ms long, peak {rss} MB, {s:.0} s on a busy machine",
            doc.words.len(),
            doc.media.duration_ms
        );
        worst = worst.max(error);
        fs::remove_dir_all(d).unwrap();
    }
    assert!(worst <= 300, "{worst}");
}

/// The contract every `Transcriber` promises, on the real thing.
#[test]
#[ignore = "needs the models and spec 027's corpus"]
fn asr_whisper_passes_the_contract() {
    let bytes = fs::read(corpus("en_5m.wav")).unwrap();
    let (data, rate, _) = wav_layout(&bytes[..4096]);
    let pcm: Vec<f32> = bytes[data as usize..data as usize + rate as usize * 2 * 20]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|s| f32::from(i16::from_le_bytes(*s)) / 32768.0)
        .collect();
    let mut resampler = Resampler::new(rate, 16_000);
    let mut out = vec![];
    resampler.push(&pcm, &mut out);
    resampler.finish(&mut out);
    let ear = memex_asr::WhisperLocal::new(&models_dir().join(memex_asr::whisper::WHISPER_FILE), 4, true).unwrap();
    memex_core::testing::transcriber_contract::run(&ear, &out, Some("en"));
}
