//! The transcription job: copies a recording to a folder, runs the worker in a child process, starts it again after a crash, and hands the rows back to be committed with the job. Saved progress lives in that folder until the job ends.
// This module spawns and waits on child processes and works in a folder on disk, which needs wall time and files.
#![allow(clippy::disallowed_methods)]
use crate::workers::{JobFailure, JobHandler, JobHold};
use memex_asr::worker::{EXIT_REFUSED, EXIT_WAITING, RESULT_FILE, SOURCE_FILE};
use memex_core::{Progress, RenditionInput, Server, ServerEdges, ServerError};
use memex_types::{JobRow, JobState, Mutation, ObjectId, ProgressUpdate, Stage, TimedDocument, VaultId};
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Crashes one recording can take before its job fails. Spec 027 saw 3 in 9.3 hours of audio.
pub const MAX_CRASHES: u32 = 8;
/// Crashes in a row with nothing saved between them: it's the file or the setup, and it won't change.
const MAX_CRASHES_WITHOUT_PROGRESS: u32 = 2;
const CHECK_EVERY: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(100);
/// How much of a worker's stderr a job keeps, as whisper.cpp can write a lot.
const STDERR_KEEP: usize = 4096;

/// How to start the worker, and the limits on it. In production it's this same binary.
#[derive(Debug, Clone)]
pub struct TranscribeCommand {
    pub program: PathBuf,
    /// Before the folder and language.
    pub args: Vec<String>,
    /// Set for the child and nothing else.
    pub envs: Vec<(String, String)>,
    /// Taken out of the child's environment. With the API off that's its key, so a worker couldn't find it even by mistake.
    pub env_remove: Vec<String>,
    pub max_rss_mb: u64,
    pub stall: Duration,
}

pub struct TranscribeHandler<E: ServerEdges> {
    pub server: Arc<Server<E>>,
    pub command: TranscribeCommand,
    /// Each vault's folder is under this, as `<root>/<vault>/transcribe/<job>`, so deleting a vault takes it too.
    pub root: PathBuf,
    /// Set when the server is stopping. The child is killed and the job goes back to wait, having lost at most the stretch it was on.
    pub stop: Arc<AtomicBool>,
}

fn permanent(message: impl Into<String>) -> JobFailure {
    JobFailure {
        message: message.into(),
        retryable: false,
        waiting: false,
    }
}

fn retry(message: impl Into<String>) -> JobFailure {
    JobFailure {
        message: message.into(),
        retryable: true,
        waiting: false,
    }
}

fn from_server(e: ServerError) -> JobFailure {
    let retryable = matches!(e, ServerError::Unavailable(_));
    JobFailure {
        message: e.to_string(),
        retryable,
        waiting: false,
    }
}

/// The bytes a job's progress takes up, which change whenever it gets anywhere. Everything in the folder counts, as decoding writes PCM and transcribing writes its log.
fn activity(dir: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(dir) else { return 0 };
    entries.flatten().filter_map(|e| e.metadata().ok()).map(|m| m.len()).sum()
}

fn progress_size(dir: &Path) -> u64 {
    activity(dir).saturating_sub(fs::metadata(dir.join(SOURCE_FILE)).map_or(0, |m| m.len()))
}

/// A process's resident memory, from `ps`, which is there on every system we run on. None if it can't say, which isn't a reason to stop anything.
fn rss_mb(pid: u32) -> Option<u64> {
    let out = Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .stderr(Stdio::null())
        .output()
        .ok()?;
    let kb: u64 = String::from_utf8_lossy(&out.stdout).trim().parse().ok()?;
    Some(kb / 1024)
}

enum Run {
    Done,
    Refused(String),
    Waiting(String),
    Crashed(String),
    Killed(JobFailure),
}

impl<E> TranscribeHandler<E>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    fn folder_root(&self, vault: &VaultId) -> PathBuf {
        self.root.join(&vault.0).join("transcribe")
    }

    /// Removes folders of jobs that have ended some other way, such as by being cancelled while queued, or dead from repeated crashes of a server. A recording's words shouldn't stay on disk after that.
    fn sweep(&self, vault: &VaultId, running: &ObjectId) {
        let Ok(entries) = fs::read_dir(self.folder_root(vault)) else { return };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name == running.0 {
                continue;
            }
            let state = self.server.job_state(vault, &ObjectId(name));
            if matches!(state, Ok(None | Some(JobState::Done | JobState::Dead | JobState::Cancelled))) {
                let _ = fs::remove_dir_all(entry.path());
            }
        }
    }

    fn copy_source(&self, vault: &VaultId, source: &ObjectId, dir: &Path) -> Result<(), JobFailure> {
        let target = dir.join(SOURCE_FILE);
        if target.exists() {
            return Ok(());
        }
        let partial = dir.join("source.part");
        let mut out = File::create(&partial).map_err(|e| retry(e.to_string()))?;
        self.server.copy_source(vault, source, &mut out).map_err(from_server)?;
        out.flush().map_err(|e| retry(e.to_string()))?;
        drop(out);
        fs::rename(&partial, &target).map_err(|e| retry(e.to_string()))
    }

    fn spawn(&self, dir: &Path, language: Option<&str>) -> Result<(Child, std::thread::JoinHandle<Vec<u8>>), JobFailure> {
        let c = &self.command;
        let mut command = Command::new(&c.program);
        command.args(&c.args).arg(dir);
        if let Some(language) = language {
            command.arg(language);
        }
        for name in &c.env_remove {
            command.env_remove(name);
        }
        let mut child = command
            .envs(c.envs.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| retry(format!("can't start the transcriber: {e}")))?;
        let mut stderr = child.stderr.take().expect("piped");
        // Only the end of it is kept, and it's read as it comes so the child can't block on a full pipe.
        let reader = std::thread::spawn(move || {
            let (mut kept, mut buffer) = (vec![], [0u8; 4096]);
            while let Ok(n) = stderr.read(&mut buffer) {
                if n == 0 {
                    break;
                }
                kept.extend_from_slice(&buffer[..n]);
                if kept.len() > STDERR_KEEP {
                    kept.drain(..kept.len() - STDERR_KEEP);
                }
            }
            kept
        });
        Ok((child, reader))
    }

    /// One run of the worker, watched until it ends.
    fn watch(&self, vault: &VaultId, job: &JobRow, dir: &Path, language: Option<&str>, hold: &JobHold) -> Result<Run, JobFailure> {
        let (mut child, stderr) = self.spawn(dir, language)?;
        let pid = child.id();
        let kill = |child: &mut Child| {
            let _ = child.kill();
            let _ = child.wait();
        };
        let mut last_check = Instant::now();
        let (mut seen, mut seen_at) = (progress_size(dir), Instant::now());
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => {}
                Err(e) => {
                    kill(&mut child);
                    return Err(retry(format!("can't wait for the transcriber: {e}")));
                }
            }
            if self.stop.load(Ordering::Relaxed) {
                kill(&mut child);
                return Ok(Run::Killed(JobFailure {
                    message: "the server is stopping".into(),
                    retryable: true,
                    waiting: true,
                }));
            }
            if hold.lost() {
                kill(&mut child);
                return Ok(Run::Killed(JobFailure::lost()));
            }
            if last_check.elapsed() >= CHECK_EVERY {
                last_check = Instant::now();
                if let Some((done, total)) = memex_asr::worker::progress_of(dir) {
                    self.server.report(
                        vault,
                        ProgressUpdate {
                            job_id: job.header.id.clone(),
                            source_id: job.input.get("source_id").and_then(|v| v.as_str()).map(|s| ObjectId(s.into())),
                            stage: Stage::Transcription,
                            done,
                            total: Some(total),
                        },
                    );
                }
                if let Some(mb) = rss_mb(pid)
                    && mb >= self.command.max_rss_mb
                {
                    kill(&mut child);
                    return Ok(Run::Killed(permanent(format!(
                        "the transcriber used {mb} MB, reaching its limit of {} MB",
                        self.command.max_rss_mb
                    ))));
                }
                let now = progress_size(dir);
                if now != seen {
                    (seen, seen_at) = (now, Instant::now());
                } else if seen_at.elapsed() >= self.command.stall {
                    kill(&mut child);
                    return Ok(Run::Crashed(format!("no progress in {} s", self.command.stall.as_secs())));
                }
            }
            std::thread::sleep(POLL);
        };
        let tail = stderr.join().unwrap_or_default();
        let message = String::from_utf8_lossy(&tail);
        let last = message
            .lines()
            .rev()
            .find(|l| !l.trim().is_empty())
            .unwrap_or("")
            .chars()
            .take(300)
            .collect::<String>();
        Ok(match status.code().map(|c| c as u8) {
            Some(0) => Run::Done,
            Some(EXIT_REFUSED) => Run::Refused(last),
            Some(EXIT_WAITING) => Run::Waiting(last),
            Some(code) => Run::Crashed(format!("the transcriber exited with {code}: {last}")),
            None => Run::Crashed(format!("the transcriber crashed: {last}")),
        })
    }

    fn transcribe(
        &self,
        vault: &VaultId,
        job: &JobRow,
        source: &ObjectId,
        dir: &Path,
        language: Option<&str>,
        hold: &JobHold,
    ) -> Result<TimedDocument, JobFailure> {
        self.copy_source(vault, source, dir)?;
        let (mut crashes, mut without_progress) = (0u32, 0u32);
        loop {
            let before = progress_size(dir);
            match self.watch(vault, job, dir, language, hold)? {
                Run::Done => {
                    let bytes = fs::read(dir.join(RESULT_FILE)).map_err(|e| retry(format!("the transcriber's result is missing: {e}")))?;
                    return serde_json::from_slice(&bytes).map_err(|e| permanent(format!("the transcriber's result isn't valid: {e}")));
                }
                Run::Refused(message) => return Err(permanent(message)),
                Run::Waiting(message) => {
                    return Err(JobFailure {
                        message,
                        retryable: true,
                        waiting: true,
                    });
                }
                Run::Killed(failure) => return Err(failure),
                Run::Crashed(message) => {
                    crashes += 1;
                    without_progress = if progress_size(dir) == before { without_progress + 1 } else { 0 };
                    tracing::warn!(job = %job.header.id, crashes, %message, "a transcriber stopped, and carries on from its saved progress");
                    if crashes > MAX_CRASHES || without_progress >= MAX_CRASHES_WITHOUT_PROGRESS {
                        return Err(permanent(format!("gave up after {crashes} crashes: {message}")));
                    }
                }
            }
        }
    }
}

impl<E> JobHandler for TranscribeHandler<E>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    fn run(&self, vault: &VaultId, job: &JobRow, hold: &JobHold) -> Result<Vec<Mutation>, JobFailure> {
        let source_id = job
            .input
            .get("source_id")
            .and_then(|v| v.as_str())
            .map(|s| ObjectId(s.into()))
            .ok_or_else(|| permanent("the job has no source"))?;
        self.sweep(vault, &job.header.id);
        let language = self.server.source_language(vault, &source_id).map_err(from_server)?;
        let dir = self.folder_root(vault).join(&job.header.id.0);
        fs::create_dir_all(&dir).map_err(|e| retry(e.to_string()))?;
        let result = self.transcribe(vault, job, &source_id, &dir, language.as_deref(), hold);
        match result {
            // Building writes blobs, so a job that has gone doesn't start it.
            Ok(_) if hold.lost() => Err(JobFailure::lost()),
            Ok(doc) => {
                let rows = self
                    .server
                    .build_rendition(vault, &source_id, RenditionInput::Timed(doc))
                    .map_err(from_server)?;
                let _ = fs::remove_dir_all(&dir);
                Ok(rows)
            }
            Err(failure) => {
                // Nothing further can use a job that won't run again.
                if !failure.retryable {
                    let _ = fs::remove_dir_all(&dir);
                }
                Err(failure)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use memex_core::testing::{ManualClock, MemBlobStore, MemRegistry, MemStore, SeededRandom};
    use memex_core::{Snapshot, Store, TRANSCRIBE_JOB, UploadLimits, UploadRequest};
    use memex_types::JobState;
    use serde_json::json;

    struct Fakes;

    impl ServerEdges for Fakes {
        type Registry = MemRegistry;
        type Store = MemStore;
        type Blobs = MemBlobStore<ManualClock>;
        type Clock = ManualClock;
        type Seed = SeededRandom;
        type Rasterizer = memex_core::testing::FakeRasterizer;
        type Lexical = memex_core::testing::MemLexical;
        type Vector = memex_core::testing::MemVector;
        type Embedder = memex_core::testing::FakeEmbedder;
        type Reranker = memex_core::testing::FakeReranker;
        type Caches = memex_core::NoCaches;
    }

    struct Rig {
        s: Arc<Server<Fakes>>,
        v: VaultId,
        root: PathBuf,
    }

    impl Drop for Rig {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    fn rig(name: &str) -> Rig {
        let s = Arc::new(Server::<Fakes>::new(
            MemRegistry::new(),
            MemStore::new(),
            MemBlobStore::new(ManualClock::starting_at(0)),
            ManualClock::starting_at(1000),
            SeededRandom::new(5),
            memex_core::testing::mem_search(),
        ));
        let c = s.authenticate(&s.bootstrap("root").unwrap().token).unwrap();
        let v = s.create_vault(&c, "v").unwrap().id;
        let req = UploadRequest {
            name: "a.wav".into(),
            trust_tier: "unverified".into(),
            metadata: json!({"language": "sv"}),
            replaces: None,
        };
        let mut bytes: &[u8] = b"RIFF\x10\0\0\0WAVEfmt the recording";
        s.upload(&c, &v, req, UploadLimits::default(), &mut bytes).unwrap();
        let root = std::env::temp_dir().join(format!("memex-transcribe-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        Rig { s, v, root }
    }

    const RESULT: &str = r#"{"parser":"p","media":{"codec":"c","sample_rate":16000,"channels":1,"duration_ms":5000,"gaps":0,"gap_ms":0,"packets":1,"bad_packets":0},"words":[{"text":"hej","start_ms":1000,"end_ms":1500}]}"#;

    impl Rig {
        /// The worker here is a shell script, given the folder as `$1`. It has the recording's bytes in `source`.
        fn handler(&self, script: &str) -> TranscribeHandler<Fakes> {
            TranscribeHandler {
                server: self.s.clone(),
                command: TranscribeCommand {
                    program: "/bin/sh".into(),
                    args: vec!["-c".into(), script.into(), "worker".into()],
                    envs: vec![("FROM_PARENT".into(), "yes".into())],
                    env_remove: vec!["MEMEX_TRANSCRIBE_API_KEY".into()],
                    max_rss_mb: 100_000,
                    stall: Duration::from_secs(60),
                },
                root: self.root.clone(),
                stop: Arc::new(AtomicBool::new(false)),
            }
        }

        fn run(&self, script: &str) -> (Result<Vec<Mutation>, JobFailure>, JobRow) {
            let job = self.s.claim_job(&self.v, TRANSCRIBE_JOB, "w").unwrap().expect("a job");
            (self.handler(script).run(&self.v, &job, &JobHold::default()), job)
        }

        fn folder(&self, job: &JobRow) -> PathBuf {
            self.root.join(&self.v.0).join("transcribe").join(&job.header.id.0)
        }
    }

    fn write_result(extra: &str) -> String {
        format!("{extra} printf '%s' '{RESULT}' > \"$1/{RESULT_FILE}\"")
    }

    #[test]
    fn a_result_becomes_rows_and_the_folder_goes() {
        let r = rig("ok");
        let (out, job) = r.run(&write_result(
            "test \"$(cat \"$1/source\" | head -c 4)\" = RIFF || exit 9; test \"$2\" = sv || exit 8; test \"$FROM_PARENT\" = yes || exit 7;",
        ));
        let rows = out.unwrap();
        assert!(
            rows.iter()
                .any(|m| matches!(m, Mutation::Put(memex_types::Row::Rendition(x)) if x.media.as_ref().is_some_and(|f| f.duration_ms == 5000)))
        );
        assert!(
            rows.iter()
                .any(|m| matches!(m, Mutation::Put(memex_types::Row::Segment(x)) if x.text == "hej" && x.time.is_some()))
        );
        assert!(!r.folder(&job).exists(), "the recording's words don't stay on disk");
    }

    #[test]
    fn a_crash_starts_it_again_and_it_carries_on() {
        let r = rig("crash");
        // Dies the first two times, writing a little each time, as a worker saving progress does.
        let script = write_result("echo x >> \"$1/progress.jsonl\"; test \"$(wc -l < \"$1/progress.jsonl\")\" -ge 3 || kill -9 $$;");
        let (out, job) = r.run(&script);
        out.unwrap();
        assert!(!r.folder(&job).exists());
    }

    #[test]
    fn crashing_with_no_progress_gives_up_soon_and_says_why() {
        let r = rig("noprogress");
        let (out, job) = r.run("echo 'GGML_ASSERT failed' >&2; kill -6 $$");
        let e = out.unwrap_err();
        assert!(!e.retryable);
        assert!(e.message.contains("gave up after 2 crashes"), "{}", e.message);
        assert!(!r.folder(&job).exists());
    }

    #[test]
    fn crashing_forever_while_getting_somewhere_still_ends() {
        let r = rig("forever");
        let (out, _) = r.run("echo x >> \"$1/progress.jsonl\"; kill -9 $$");
        let e = out.unwrap_err();
        assert!(!e.retryable);
        assert!(e.message.contains(&format!("gave up after {} crashes", MAX_CRASHES + 1)), "{}", e.message);
    }

    #[test]
    fn a_refusal_fails_once_with_its_reason() {
        let r = rig("refused");
        let (out, job) = r.run("echo 'not a readable recording' >&2; exit 3");
        let e = out.unwrap_err();
        assert_eq!((e.message.as_str(), e.retryable, e.waiting), ("not a readable recording", false, false));
        assert!(!r.folder(&job).exists());
    }

    #[test]
    fn a_provider_away_waits_and_keeps_its_progress() {
        let r = rig("waiting");
        let (out, job) = r.run("echo saved > \"$1/progress.jsonl\"; echo 'the provider answered 503' >&2; exit 4");
        let e = out.unwrap_err();
        assert!(e.waiting && e.retryable, "{e:?}");
        assert!(r.folder(&job).join("progress.jsonl").exists());
    }

    #[test]
    fn nonsense_in_the_result_fails_cleanly() {
        let r = rig("nonsense");
        let (out, _) = r.run(&format!("echo nope > \"$1/{RESULT_FILE}\""));
        assert!(out.unwrap_err().message.contains("isn't valid"));
    }

    #[test]
    fn a_hang_is_killed_at_its_limit() {
        let r = rig("hang");
        let job = r.s.claim_job(&r.v, TRANSCRIBE_JOB, "w").unwrap().unwrap();
        let mut h = r.handler("sleep 60");
        h.command.stall = Duration::from_millis(1);
        let started = Instant::now();
        let e = h.run(&r.v, &job, &JobHold::default()).unwrap_err();
        assert!(e.message.contains("no progress"), "{}", e.message);
        assert!(started.elapsed() < Duration::from_secs(40), "{:?}", started.elapsed());
    }

    #[test]
    fn over_its_memory_limit_a_worker_is_killed_and_the_job_fails_once() {
        let r = rig("rss");
        let job = r.s.claim_job(&r.v, TRANSCRIBE_JOB, "w").unwrap().unwrap();
        let mut h = r.handler("sleep 60");
        h.command.max_rss_mb = 0;
        let e = h.run(&r.v, &job, &JobHold::default()).unwrap_err();
        assert!(e.message.contains("reaching its limit"), "{}", e.message);
        assert!(!e.retryable);
    }

    #[test]
    fn a_stopping_server_kills_the_child_and_the_job_waits() {
        let r = rig("stop");
        let job = r.s.claim_job(&r.v, TRANSCRIBE_JOB, "w").unwrap().unwrap();
        let h = r.handler("sleep 60");
        h.stop.store(true, Ordering::Relaxed);
        let started = Instant::now();
        let e = h.run(&r.v, &job, &JobHold::default()).unwrap_err();
        assert!(e.waiting, "{e:?}");
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn a_cancelled_job_kills_the_child_and_builds_nothing() {
        let r = rig("cancelled");
        let job = r.s.claim_job(&r.v, TRANSCRIBE_JOB, "w").unwrap().unwrap();
        let hold = JobHold::default();
        crate::workers::lose_for_tests(&hold);
        let started = Instant::now();
        let e = r.handler("sleep 60").run(&r.v, &job, &hold).unwrap_err();
        assert_eq!(e.message, "this worker no longer holds the job");
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn a_job_lost_as_the_child_finishes_builds_nothing() {
        let r = rig("lostlate");
        let job = r.s.claim_job(&r.v, TRANSCRIBE_JOB, "w").unwrap().unwrap();
        let hold = JobHold::default();
        crate::workers::lose_for_tests(&hold);
        // It only finishes if it's quicker than the first look at the hold, so either way there are no rows.
        assert!(r.handler(&write_result("")).run(&r.v, &job, &hold).is_err());
    }

    #[test]
    fn a_folder_left_by_an_ended_job_is_swept() {
        let r = rig("sweep");
        let job = r.s.claim_job(&r.v, TRANSCRIBE_JOB, "w").unwrap().unwrap();
        let stale = r.root.join(&r.v.0).join("transcribe").join("no-such-job");
        fs::create_dir_all(&stale).unwrap();
        fs::write(stale.join("progress.jsonl"), "words").unwrap();
        r.handler(&write_result("")).run(&r.v, &job, &JobHold::default()).unwrap();
        assert!(!stale.exists());
    }

    #[test]
    fn a_recording_goes_to_transcription_and_a_document_does_not() {
        let r = rig("kinds");
        let snap = r.s.engine().store().snapshot(&r.v).unwrap();
        assert_eq!(snap.jobs(Some(TRANSCRIBE_JOB), Some(JobState::Queued), None, 10).unwrap().len(), 1);
        assert_eq!(snap.jobs(Some(memex_core::RENDITION_JOB), None, None, 10).unwrap().len(), 0);
    }
}
