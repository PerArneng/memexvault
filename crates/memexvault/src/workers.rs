//! The worker pool: threads separate from request handling, with a limit per job kind.
// Workers wait on wall time between polls, which is this edge's job.
#![allow(clippy::disallowed_methods)]
use memex_core::{Server, ServerEdges, StepOutcome};
use memex_types::{JobRow, Mutation, VaultId};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

/// Why a job failed. `retryable: false` means trying again can't help.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobFailure {
    pub message: String,
    pub retryable: bool,
    /// Something it needs is away, so it goes back to wait and uses up no attempts.
    pub waiting: bool,
}

/// Does one job's work. Results come back as mutations, committed with the job's completion.
pub trait JobHandler: Send + Sync {
    fn run(&self, vault: &VaultId, job: &JobRow) -> Result<Vec<Mutation>, JobFailure>;
}

/// One kind of job and how many may run at once.
pub struct Pool {
    pub kind: String,
    pub workers: usize,
    pub handler: Arc<dyn JobHandler>,
}

/// Runs at most one job. Returns whether it found one, so the caller knows whether to wait.
pub fn run_one<E: ServerEdges>(server: &Server<E>, handler: &dyn JobHandler, kind: &str, owner: &str) -> bool {
    let vaults = match server.vault_ids() {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "can't list vaults");
            return false;
        }
    };
    for vault in vaults {
        let job = match server.claim_job(&vault, kind, owner) {
            Ok(Some(j)) => j,
            Ok(None) => continue,
            Err(e) => {
                tracing::warn!(error = %e, "can't claim a job");
                continue;
            }
        };
        // A panic in a handler is that job's failure, and doesn't take the worker with it.
        let outcome = catch_unwind(AssertUnwindSafe(|| handler.run(&vault, &job))).unwrap_or_else(|_| {
            Err(JobFailure {
                message: "the handler panicked".into(),
                retryable: true,
                waiting: false,
            })
        });
        // Whatever came of it, there's nothing more to say about this attempt.
        memex_core::Progress::finished(server, &vault, &job.header.id);
        let recorded = match outcome {
            Ok(results) => server.complete_job(&vault, &job, results),
            Err(f) if f.waiting => server.wait_job(&vault, &job, &f.message),
            Err(f) => server.fail_job(&vault, &job, &f.message, f.retryable),
        };
        match recorded {
            Ok(true) => {}
            // It lost the job to lease expiry or cancellation, and wrote nothing.
            Ok(false) => tracing::info!(job = %job.header.id, "job no longer held"),
            Err(e) => tracing::warn!(job = %job.header.id, error = %e, "can't record the job's result, so its lease will expire and it will run again"),
        }
        return true;
    }
    false
}

/// Starts the pool's threads. They stop soon after `stop` is set.
pub fn start<E>(server: Arc<Server<E>>, pools: Vec<Pool>, stop: Arc<AtomicBool>) -> Vec<JoinHandle<()>>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    let mut threads = vec![];
    for pool in pools {
        for n in 0..pool.workers {
            let (server, handler, stop, kind) = (server.clone(), pool.handler.clone(), stop.clone(), pool.kind.clone());
            let owner = format!("{kind}-{}-{n}", std::process::id());
            threads.push(std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    if !run_one(&server, handler.as_ref(), &kind, &owner) {
                        std::thread::sleep(Duration::from_millis(100));
                    }
                }
            }));
        }
    }
    threads
}

/// Creates embeddings. Results go to the embedding store rather than the job, since replaying the log must not need them.
pub struct EmbedHandler<E: ServerEdges> {
    pub server: Arc<Server<E>>,
}

impl<E> JobHandler for EmbedHandler<E>
where
    E: ServerEdges,
    Server<E>: Send + Sync,
{
    fn run(&self, vault: &VaultId, job: &JobRow) -> Result<Vec<Mutation>, JobFailure> {
        self.server.run_embed_job(vault, job).map(|_| vec![]).map_err(|e| JobFailure {
            message: e.message,
            retryable: e.retryable,
            waiting: false,
        })
    }
}

/// Runs extraction with whichever agent the composition root built.
pub struct ExtractHandler<E: ServerEdges, A> {
    pub server: Arc<Server<E>>,
    pub agent: A,
}

impl<E, A> JobHandler for ExtractHandler<E, A>
where
    E: ServerEdges,
    Server<E>: Send + Sync,
    A: memex_core::AgentRunner + Send + Sync,
{
    fn run(&self, vault: &VaultId, job: &JobRow) -> Result<Vec<Mutation>, JobFailure> {
        self.server.run_extraction_job(vault, job, &self.agent).map_err(job_failure)
    }
}

fn job_failure(e: memex_core::ExtractJobError) -> JobFailure {
    match e {
        memex_core::ExtractJobError::Wait(message) => JobFailure {
            message,
            retryable: true,
            waiting: true,
        },
        memex_core::ExtractJobError::Retry(message) => JobFailure {
            message,
            retryable: true,
            waiting: false,
        },
        memex_core::ExtractJobError::Fatal(message) => JobFailure {
            message,
            retryable: false,
            waiting: false,
        },
        // Recording the result finds out the job has gone, and writes nothing.
        memex_core::ExtractJobError::Lost => JobFailure {
            message: "this worker no longer holds the job".into(),
            retryable: true,
            waiting: false,
        },
    }
}

/// Runs the reviewer agent. Its verdict commits with the job, and it can write nothing else.
pub struct ReviewHandler<E: ServerEdges, A> {
    pub server: Arc<Server<E>>,
    pub agent: A,
}

impl<E, A> JobHandler for ReviewHandler<E, A>
where
    E: ServerEdges,
    Server<E>: Send + Sync,
    A: memex_core::AgentRunner + Send + Sync,
{
    fn run(&self, vault: &VaultId, job: &JobRow) -> Result<Vec<Mutation>, JobFailure> {
        self.server.run_review_job(vault, job, &self.agent).map_err(job_failure)
    }
}

/// Runs an audit. With no agent it still runs the deterministic checks, and nothing leaves the server.
pub struct AuditHandler<E: ServerEdges, A> {
    pub server: Arc<Server<E>>,
    pub agent: Option<A>,
}

impl<E, A> JobHandler for AuditHandler<E, A>
where
    E: ServerEdges,
    Server<E>: Send + Sync,
    A: memex_core::AgentRunner + Send + Sync,
{
    fn run(&self, vault: &VaultId, job: &JobRow) -> Result<Vec<Mutation>, JobFailure> {
        self.server.run_audit_job(vault, job, self.agent.as_ref()).map_err(job_failure)
    }
}

/// Starts each vault's scheduled audits when the clock says so. Its own thread, as nothing else here should wait on it.
pub fn start_audit_scheduler<E>(server: Arc<Server<E>>, stop: Arc<AtomicBool>) -> JoinHandle<()>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    std::thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            if let Err(e) = server.audit_tick_all() {
                tracing::warn!(error = %e, "can't check the audit schedules");
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    })
}

/// Keeps every vault's vector index following its log. It has its own thread, since building a graph can take a while and keyword search shouldn't lag behind that.
pub fn start_vector_projector<E>(server: Arc<Server<E>>, stop: Arc<AtomicBool>) -> JoinHandle<()>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    std::thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            let pause = step_all(&server, Server::project_vector, "vector index");
            std::thread::sleep(pause);
        }
    })
}

/// Keeps every vault's keyword index following its log, in one thread of its own.
pub fn start_projector<E>(server: Arc<Server<E>>, stop: Arc<AtomicBool>) -> JoinHandle<()>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    std::thread::spawn(move || {
        while !stop.load(Ordering::Relaxed) {
            let pause = step_all(&server, Server::project_lexical, "keyword index");
            std::thread::sleep(pause);
        }
    })
}

/// Steps each vault once, and says how long to wait before the next round.
fn step_all<E: ServerEdges>(server: &Server<E>, step: impl Fn(&Server<E>, &VaultId) -> Result<StepOutcome, memex_core::ProjectError>, what: &str) -> Duration {
    let vaults = match server.vault_ids() {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(error = %e, "can't list vaults");
            return Duration::from_secs(1);
        }
    };
    let (mut busy, mut failed) = (false, false);
    for vault in vaults {
        match step(server, &vault) {
            Ok(StepOutcome::Progress) => busy = true,
            Ok(StepOutcome::Idle) => {}
            Err(e) => {
                tracing::warn!(vault = %vault, error = %e, "{what} step failed, and will be retried");
                failed = true;
            }
        }
    }
    if failed {
        Duration::from_secs(1)
    } else if busy {
        Duration::ZERO
    } else {
        Duration::from_millis(50)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use memex_core::testing::{ManualClock, MemBlobStore, MemRegistry, MemStore, SeededRandom};
    use memex_core::{Snapshot, Store, UploadLimits, UploadRequest};
    use memex_types::{Header, JobState, ObjectId, PageRow, Row, Status};
    use serde_json::json;
    use std::sync::atomic::AtomicU32;

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

    fn setup(n: u32) -> (Arc<Server<Fakes>>, memex_types::VaultId) {
        let (s, v, _) = setup_with_caller(n);
        (s, v)
    }

    fn setup_with_caller(n: u32) -> (Arc<Server<Fakes>>, memex_types::VaultId, memex_core::Caller) {
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
        for i in 0..n {
            let req = UploadRequest {
                name: "a.pdf".into(),
                trust_tier: "unverified".into(),
                metadata: json!({}),
                replaces: None,
            };
            s.upload(&c, &v, req, UploadLimits::default(), &mut format!("%PDF-1.7 {i}").as_bytes()).unwrap();
        }
        (s, v, c)
    }

    fn count(s: &Server<Fakes>, v: &memex_types::VaultId, state: JobState) -> usize {
        s.engine().store().snapshot(v).unwrap().jobs(None, Some(state), None, 100_000).unwrap().len()
    }

    struct Derives(AtomicU32);

    impl JobHandler for Derives {
        fn run(&self, _: &VaultId, job: &JobRow) -> Result<Vec<Mutation>, JobFailure> {
            self.0.fetch_add(1, Ordering::SeqCst);
            let header = Header {
                id: ObjectId(format!("d-{}", job.header.id)),
                creator: "w".into(),
                status: Status::Active,
                version: 1,
                acl_id: None,
            };
            Ok(vec![Mutation::Put(Row::Page(PageRow {
                header,
                title: "t".into(),
                body: "b".into(),
                updated_at_ms: 0,
                slug: String::new(),
                redirects: vec![],
            }))])
        }
    }

    struct Panics;

    impl JobHandler for Panics {
        fn run(&self, _: &VaultId, _: &JobRow) -> Result<Vec<Mutation>, JobFailure> {
            panic!("a bug in a handler");
        }
    }

    struct Refuses;

    impl JobHandler for Refuses {
        fn run(&self, _: &VaultId, _: &JobRow) -> Result<Vec<Mutation>, JobFailure> {
            Err(JobFailure {
                message: "not parseable".into(),
                retryable: false,
                waiting: false,
            })
        }
    }

    #[test]
    fn many_workers_finish_every_job_exactly_once() {
        let (s, v) = setup(300);
        let handler = Arc::new(Derives(AtomicU32::new(0)));
        let stop = Arc::new(AtomicBool::new(false));
        let threads = start(
            s.clone(),
            vec![Pool {
                kind: "rendition".into(),
                workers: 6,
                handler: handler.clone(),
            }],
            stop.clone(),
        );
        for _ in 0..500 {
            if handler.0.load(Ordering::SeqCst) >= 300 && count(&s, &v, JobState::Done) == 300 {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        stop.store(true, Ordering::SeqCst);
        for t in threads {
            t.join().unwrap();
        }
        assert_eq!(handler.0.load(Ordering::SeqCst), 300);
        assert_eq!(count(&s, &v, JobState::Done), 300);
    }

    #[test]
    fn a_panic_is_one_job_s_failure_and_the_worker_goes_on() {
        let (s, v) = setup(1);
        assert!(run_one(s.as_ref(), &Panics, "rendition", "w"));
        let dead_or_retry = count(&s, &v, JobState::Queued);
        assert_eq!(dead_or_retry, 1, "it goes back to the queue with a delay");
        assert!(!run_one(s.as_ref(), &Panics, "rendition", "w"), "the delay holds it back");
    }

    #[test]
    fn a_permanent_failure_goes_dead_at_once() {
        let (s, v) = setup(1);
        assert!(run_one(s.as_ref(), &Refuses, "rendition", "w"));
        assert_eq!(count(&s, &v, JobState::Dead), 1);
    }

    fn text_upload(s: &Server<Fakes>, c: &memex_core::Caller, v: &VaultId) -> memex_types::ObjectId {
        let text = "Fartyget byggdes i Stockholm. Det är en mening om fartyget.";
        let req = UploadRequest {
            name: "a.txt".into(),
            trust_tier: "unverified".into(),
            metadata: json!({}),
            replaces: None,
        };
        let source = s.upload(c, v, req, UploadLimits::default(), &mut text.as_bytes()).unwrap().source.header.id;
        let job = s.claim_job(v, memex_core::RENDITION_JOB, "p").unwrap().unwrap();
        let rows = s
            .build_rendition(
                v,
                &source,
                memex_core::RenditionInput::Plain {
                    text: text.into(),
                    parser: "plain-1".into(),
                },
            )
            .unwrap();
        assert!(s.complete_job(v, &job, rows).unwrap());
        source
    }

    #[test]
    fn an_outage_through_the_pool_waits_and_the_next_reply_finishes_the_job() {
        let (s, v, c) = setup_with_caller(0);
        s.set_extraction(Some(memex_core::ExtractionLimits::default()));
        let source = text_upload(&s, &c, &v);
        let script = vec![
            Err(memex_types::AgentError::Unavailable("down".into())),
            Ok(memex_types::AgentReply {
                json: json!({"claims": [{"segment_id": "s1", "quote": "Fartyget byggdes i Stockholm.", "subject": "Fartyget", "predicate": "byggdes i", "value": "Stockholm"}], "entities": {}}).to_string(),
                usage: memex_types::AgentUsage::default(),
                tool_calls: 0,
            }),
        ];
        let handler = ExtractHandler {
            server: s.clone(),
            agent: memex_core::testing::ScriptedAgent::new(script),
        };
        assert!(run_one(s.as_ref(), &handler, memex_core::EXTRACT_JOB, "w"));
        assert_eq!(s.extraction_status(&c, &v, &source).unwrap().state, memex_core::ExtractionState::Waiting);
        assert!(!run_one(s.as_ref(), &handler, memex_core::EXTRACT_JOB, "w"), "it holds back until due");
        s.engine().clock().advance(Duration::from_secs(600));
        assert!(run_one(s.as_ref(), &handler, memex_core::EXTRACT_JOB, "w"));
        assert_eq!(s.extraction_status(&c, &v, &source).unwrap().state, memex_core::ExtractionState::Done);
    }

    #[test]
    fn no_work_returns_false() {
        let (s, _) = setup(0);
        assert!(!run_one(s.as_ref(), &Panics, "rendition", "w"));
    }
}
