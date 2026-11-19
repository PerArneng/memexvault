//! The worker pool: threads separate from request handling, with a limit per job kind.
// Workers wait on wall time between polls, which is this edge's job.
#![allow(clippy::disallowed_methods)]
use memex_core::{Server, ServerEdges, StepOutcome};
use memex_types::{JobRow, Mutation, VaultId};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{RecvTimeoutError, channel};
use std::thread::JoinHandle;
use std::time::Duration;

/// How often a worker looks at the clock while its handler runs. The clock decides when to renew, and this is only how soon it notices.
const KEEPER_POLL: Duration = Duration::from_millis(250);
/// How long a worker waits to try again after the store failed a renewal.
const RENEW_RETRY_MS: i64 = 2_000;

/// The least time between two lines of the same warning from the poll loop, across all worker threads.
const QUIET_MS: i64 = 10_000;

/// Lets one warning through at most once per `QUIET_MS`, process wide, and counts the ones it held back. Every worker thread polls every vault ten times a second, so a database that's away would otherwise fill the log.
struct Quiet {
    last_ms: AtomicI64,
    held: AtomicU64,
}

impl Quiet {
    const fn new() -> Self {
        Self {
            last_ms: AtomicI64::new(i64::MIN),
            held: AtomicU64::new(0),
        }
    }

    /// Some(how many were held back since the last line) when this one may be logged.
    fn allow(&self, now_ms: i64) -> Option<u64> {
        let last = self.last_ms.load(Ordering::Relaxed);
        if now_ms.saturating_sub(last) >= QUIET_MS && self.last_ms.compare_exchange(last, now_ms, Ordering::Relaxed, Ordering::Relaxed).is_ok() {
            return Some(self.held.swap(0, Ordering::Relaxed));
        }
        self.held.fetch_add(1, Ordering::Relaxed);
        None
    }
}

static LIST_QUIET: Quiet = Quiet::new();
static CLAIM_QUIET: Quiet = Quiet::new();
static AUDIT_QUIET: Quiet = Quiet::new();
static STEP_QUIET: Quiet = Quiet::new();

/// Why a job failed. `retryable: false` means trying again can't help.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobFailure {
    pub message: String,
    pub retryable: bool,
    /// Something it needs is away, so it goes back to wait and uses up no attempts.
    pub waiting: bool,
}

impl JobFailure {
    /// The job was cancelled or taken while it ran. Whatever comes back after this, the fence on completion writes nothing.
    pub fn lost() -> Self {
        Self {
            message: "this worker no longer holds the job".into(),
            retryable: true,
            waiting: false,
        }
    }
}

/// Tells a running handler whether its job is still its own. A handler that can stop part way checks this and stops.
#[derive(Debug, Default)]
pub struct JobHold {
    lost: AtomicBool,
}

impl JobHold {
    pub fn lost(&self) -> bool {
        self.lost.load(Ordering::Relaxed)
    }
}

/// For a test that needs a handler to find its job has gone.
#[cfg(test)]
pub(crate) fn lose_for_tests(hold: &JobHold) {
    hold.lost.store(true, Ordering::Relaxed);
}

/// Does one job's work. Results come back as mutations, committed with the job's completion. The worker keeps the job's lease while this runs, whatever kind of job it is.
pub trait JobHandler: Send + Sync {
    fn run(&self, vault: &VaultId, job: &JobRow, hold: &JobHold) -> Result<Vec<Mutation>, JobFailure>;
}

/// Handler threads left running past their job's ceiling, in this process. Each still holds a thread and whatever its handler holds.
static ABANDONED: AtomicUsize = AtomicUsize::new(0);

/// How many handlers this process has given up on at their ceiling and that haven't returned since.
pub fn abandoned_handlers() -> usize {
    ABANDONED.load(Ordering::Relaxed)
}

/// What came of running a handler under its ceiling.
enum Held {
    Finished(Result<Vec<Mutation>, JobFailure>),
    /// It ran past its ceiling and was left running. Nothing it returns is read.
    PastCeiling,
}

/// Where a handler's thread puts its result, unless the worker has stopped waiting for it.
struct Handoff {
    abandoned: Mutex<bool>,
}

/// Runs the handler on a thread of its own, while this thread keeps the job's lease, renewing it every `RENEW_EVERY_MS` counted from the claim, by the server's clock. Renewing stops at the first answer that the job has gone, and the handler is told through its `JobHold`. A store that's away isn't that: it tries again, and the fence on completion decides whether the work counts.
///
/// At `ceiling` from the claim it stops renewing and stops waiting: the handler is told the job is lost, and its thread is left to end on its own, as a thread can't be killed. Whatever it returns then is thrown away. The worker is free at once for the next job. Renewals and the result are on this one thread, so no renewal can land after the result is recorded.
fn run_held<E, H>(server: &Arc<Server<E>>, handler: &Arc<H>, vault: &VaultId, job: &JobRow, ceiling: Duration) -> Held
where
    E: ServerEdges + 'static,
    Server<E>: Send + Sync,
    H: JobHandler + ?Sized + 'static,
{
    use memex_core::Clock as _;
    let hold = Arc::new(JobHold::default());
    let handoff = Arc::new(Handoff { abandoned: Mutex::new(false) });
    let (send, result) = channel::<Result<Vec<Mutation>, JobFailure>>();
    let spawned = std::thread::Builder::new().name(format!("job-{}", job.kind)).spawn({
        let (handler, vault, job, hold, handoff) = (handler.clone(), vault.clone(), job.clone(), hold.clone(), handoff.clone());
        move || {
            // A panic in a handler is that job's failure, and doesn't take the worker with it.
            let outcome = catch_unwind(AssertUnwindSafe(|| handler.run(&vault, &job, &hold))).unwrap_or_else(|_| {
                Err(JobFailure {
                    message: "the handler panicked".into(),
                    retryable: true,
                    waiting: false,
                })
            });
            let abandoned = handoff.abandoned.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if *abandoned {
                let left = ABANDONED.fetch_sub(1, Ordering::Relaxed) - 1;
                tracing::warn!(job = %job.header.id, still_running = left, "a handler given up on at its ceiling has returned, and its result is thrown away");
            } else {
                let _ = send.send(outcome);
            }
        }
    });
    if let Err(e) = spawned {
        return Held::Finished(Err(JobFailure {
            message: format!("can't start the thread that runs the job: {e}"),
            retryable: true,
            waiting: false,
        }));
    }
    let clock = server.engine().clock();
    // Counted from the claim, as this can start late on a busy machine.
    let claimed_at = job.lease_until_ms.map_or_else(|| clock.now_ms(), |until| until - memex_core::jobs::LEASE_MS);
    let ceiling_at = claimed_at.saturating_add(i64::try_from(ceiling.as_millis()).unwrap_or(i64::MAX));
    let mut next_at = claimed_at + memex_core::jobs::RENEW_EVERY_MS;
    let mut renewing = true;
    // One line when renewals start failing and one when they work again, however long the store is away.
    let mut failing = false;
    loop {
        match result.recv_timeout(KEEPER_POLL) {
            Ok(outcome) => return Held::Finished(outcome),
            // It can't end without sending, short of a bug.
            Err(RecvTimeoutError::Disconnected) => {
                return Held::Finished(Err(JobFailure {
                    message: "the handler's thread ended without a result".into(),
                    retryable: true,
                    waiting: false,
                }));
            }
            Err(RecvTimeoutError::Timeout) => {}
        }
        let now = clock.now_ms();
        if now >= ceiling_at {
            let mut abandoned = handoff.abandoned.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            // It may have finished in the moment before.
            if let Ok(outcome) = result.try_recv() {
                return Held::Finished(outcome);
            }
            *abandoned = true;
            hold.lost.store(true, Ordering::Relaxed);
            let left = ABANDONED.fetch_add(1, Ordering::Relaxed) + 1;
            tracing::warn!(job = %job.header.id, kind = %job.kind, ceiling_s = ceiling.as_secs(), still_running = left, "a job ran past its ceiling, so its worker gives up on it and leaves its handler's thread to end on its own");
            return Held::PastCeiling;
        }
        if !renewing || now < next_at {
            continue;
        }
        match server.hold_job(vault, job, false, vec![]) {
            Ok(true) => {
                if failing {
                    tracing::info!(job = %job.header.id, "renewed the job's lease again");
                    failing = false;
                }
                next_at = now + memex_core::jobs::RENEW_EVERY_MS;
            }
            Ok(false) | Err(memex_core::ServerError::NotFound) => {
                tracing::info!(job = %job.header.id, "the job was cancelled or taken, so its handler stops");
                hold.lost.store(true, Ordering::Relaxed);
                renewing = false;
            }
            Err(e) => {
                if !failing {
                    tracing::warn!(job = %job.header.id, error = %e, "can't renew a job's lease, and will keep trying");
                    failing = true;
                }
                next_at = now + RENEW_RETRY_MS;
            }
        }
    }
}

/// The reason recorded on an attempt that ran past its ceiling.
pub fn past_ceiling_message(kind: &str, ceiling: Duration) -> String {
    let setting = memex_core::JobCeilings::setting(kind).map_or_else(String::new, |name| format!(", set by {name}"));
    format!(
        "the job ran past its ceiling of {} s for {kind} jobs{setting}, so its worker gave up on it",
        ceiling.as_secs()
    )
}

/// One kind of job, how many may run at once, and how long one attempt may take.
pub struct Pool {
    pub kind: String,
    pub workers: usize,
    pub handler: Arc<dyn JobHandler>,
    pub ceiling: Duration,
}

/// Runs at most one job, giving up on it at `ceiling`. Returns whether it found one, so the caller knows whether to wait.
pub fn run_one<E, H>(server: &Arc<Server<E>>, handler: &Arc<H>, kind: &str, owner: &str, ceiling: Duration) -> bool
where
    E: ServerEdges + 'static,
    Server<E>: Send + Sync,
    H: JobHandler + ?Sized + 'static,
{
    use memex_core::Clock as _;
    let now_ms = || server.engine().clock().now_ms();
    let vaults = match server.vault_ids() {
        Ok(v) => v,
        Err(e) => {
            if let Some(held) = LIST_QUIET.allow(now_ms()) {
                tracing::warn!(error = %e, held_back = held, "can't list vaults");
            }
            return false;
        }
    };
    for vault in vaults {
        let job = match server.claim_job(&vault, kind, owner) {
            Ok(Some(j)) => j,
            Ok(None) => continue,
            // The server says once that the vault's lease was lost and once that it's back.
            Err(_) if server.lease_lost(&vault) => continue,
            Err(e) => {
                if let Some(held) = CLAIM_QUIET.allow(now_ms()) {
                    tracing::warn!(vault = %vault, error = %e, held_back = held, "can't claim a job");
                }
                continue;
            }
        };
        let outcome = run_held(server, handler, &vault, &job, ceiling);
        // Whatever came of it, there's nothing more to say about this attempt.
        memex_core::Progress::finished(server.as_ref(), &vault, &job.header.id);
        let recorded = match outcome {
            Held::Finished(Ok(results)) => server.complete_job(&vault, &job, results),
            Held::Finished(Err(f)) if f.waiting => server.wait_job(&vault, &job, &f.message),
            Held::Finished(Err(f)) => server.fail_job(&vault, &job, &f.message, f.retryable),
            // Another attempt may well finish: extraction and audits carry on from what they committed, and a hang may not come again.
            Held::PastCeiling => server.fail_job(&vault, &job, &past_ceiling_message(kind, ceiling), true),
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
            let (server, handler, stop, kind, ceiling) = (server.clone(), pool.handler.clone(), stop.clone(), pool.kind.clone(), pool.ceiling);
            let owner = format!("{kind}-{}-{n}", std::process::id());
            threads.push(std::thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    if !run_one(&server, &handler, &kind, &owner, ceiling) {
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
    fn run(&self, vault: &VaultId, job: &JobRow, hold: &JobHold) -> Result<Vec<Mutation>, JobFailure> {
        self.server
            .run_embed_job_while(vault, job, &|| !hold.lost())
            .map(|_| vec![])
            .map_err(|e| JobFailure {
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
    fn run(&self, vault: &VaultId, job: &JobRow, _: &JobHold) -> Result<Vec<Mutation>, JobFailure> {
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
        memex_core::ExtractJobError::Lost => JobFailure::lost(),
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
    fn run(&self, vault: &VaultId, job: &JobRow, _: &JobHold) -> Result<Vec<Mutation>, JobFailure> {
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
    fn run(&self, vault: &VaultId, job: &JobRow, _: &JobHold) -> Result<Vec<Mutation>, JobFailure> {
        self.server.run_audit_job(vault, job, self.agent.as_ref()).map_err(job_failure)
    }
}

/// Does an approved erasure's physical work: index files, blobs, chat threads, caches and compaction (spec 030).
pub struct EraseHandler<E: ServerEdges> {
    pub server: Arc<Server<E>>,
}

impl<E> JobHandler for EraseHandler<E>
where
    E: ServerEdges,
    Server<E>: Send + Sync,
{
    fn run(&self, vault: &VaultId, job: &JobRow, _: &JobHold) -> Result<Vec<Mutation>, JobFailure> {
        self.server.run_erase_job(vault, job).map_err(job_failure)
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
                use memex_core::Clock as _;
                if let Some(held) = AUDIT_QUIET.allow(server.engine().clock().now_ms()) {
                    tracing::warn!(error = %e, held_back = held, "can't check the audit schedules");
                }
            }
            std::thread::sleep(Duration::from_secs(1));
        }
    })
}

/// Sweeps every vault's orphan blobs on a schedule (spec 030): the first sweep one interval after startup, as a restart shouldn't cost a scan.
pub fn start_sweeper<E>(server: Arc<Server<E>>, stop: Arc<AtomicBool>, sweep: memex_core::SweepSettings) -> JoinHandle<()>
where
    E: ServerEdges + Send + Sync + 'static,
    Server<E>: Send + Sync,
{
    std::thread::spawn(move || {
        let mut waited = 0;
        while !stop.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_secs(1));
            waited += 1;
            if waited < sweep.interval_s {
                continue;
            }
            waited = 0;
            match server.sweep_all(sweep.grace_s as i64 * 1000) {
                Ok(reports) => {
                    for (vault, r) in reports {
                        if r.removed > 0 {
                            tracing::info!(vault = %vault, removed = r.removed, bytes = r.removed_bytes, "swept orphan blobs");
                        }
                    }
                }
                Err(e) => tracing::warn!(error = %e, "the orphan sweep failed, and runs again at its next interval"),
            }
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
    use memex_core::Clock as _;
    let now_ms = || server.engine().clock().now_ms();
    let vaults = match server.vault_ids() {
        Ok(v) => v,
        Err(e) => {
            if let Some(held) = LIST_QUIET.allow(now_ms()) {
                tracing::warn!(error = %e, held_back = held, "can't list vaults");
            }
            return Duration::from_secs(1);
        }
    };
    let (mut busy, mut failed) = (false, false);
    for vault in vaults {
        match step(server, &vault) {
            Ok(StepOutcome::Progress) => busy = true,
            Ok(StepOutcome::Idle) => {}
            Err(e) => {
                if let Some(held) = STEP_QUIET.allow(now_ms()) {
                    tracing::warn!(vault = %vault, error = %e, held_back = held, "{what} step failed, and will be retried");
                }
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

    #[test]
    fn quiet_lets_one_line_through_per_interval_and_counts_the_rest() {
        let q = Quiet::new();
        assert_eq!(q.allow(1_000), Some(0));
        let held = (1..100).filter(|i| q.allow(1_000 + i * 100).is_some()).count();
        assert_eq!(held, 0, "within 10 s of the first");
        assert_eq!(q.allow(1_000 + QUIET_MS), Some(99));
        // A minute of a thread failing ten times a second: 6 lines.
        let q = Quiet::new();
        let lines = (0..600).filter(|i| q.allow(i * 100).is_some()).count();
        assert_eq!(lines, 6);
    }
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

    /// One worker's turn, with the default ceiling for its kind.
    fn run<H: JobHandler + 'static>(s: &Arc<Server<Fakes>>, handler: &Arc<H>, kind: &str, owner: &str) -> bool {
        run_one(s, handler, kind, owner, memex_core::JobCeilings::default().for_kind(kind))
    }

    fn count(s: &Server<Fakes>, v: &memex_types::VaultId, state: JobState) -> usize {
        s.engine().store().snapshot(v).unwrap().jobs(None, Some(state), None, 100_000).unwrap().len()
    }

    struct Derives(AtomicU32);

    impl JobHandler for Derives {
        fn run(&self, _: &VaultId, job: &JobRow, _: &JobHold) -> Result<Vec<Mutation>, JobFailure> {
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
        fn run(&self, _: &VaultId, _: &JobRow, _: &JobHold) -> Result<Vec<Mutation>, JobFailure> {
            panic!("a bug in a handler");
        }
    }

    struct Refuses;

    impl JobHandler for Refuses {
        fn run(&self, _: &VaultId, _: &JobRow, _: &JobHold) -> Result<Vec<Mutation>, JobFailure> {
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
                ceiling: memex_core::JobCeilings::default().for_kind("rendition"),
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
        assert!(run(&s, &Arc::new(Panics), "rendition", "w"));
        let dead_or_retry = count(&s, &v, JobState::Queued);
        assert_eq!(dead_or_retry, 1, "it goes back to the queue with a delay");
        assert!(!run(&s, &Arc::new(Panics), "rendition", "w"), "the delay holds it back");
    }

    #[test]
    fn a_permanent_failure_goes_dead_at_once() {
        let (s, v) = setup(1);
        assert!(run(&s, &Arc::new(Refuses), "rendition", "w"));
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
        let handler = Arc::new(ExtractHandler {
            server: s.clone(),
            agent: memex_core::testing::ScriptedAgent::new(script),
        });
        assert!(run(&s, &handler, memex_core::EXTRACT_JOB, "w"));
        assert_eq!(s.extraction_status(&c, &v, &source).unwrap().state, memex_core::ExtractionState::Waiting);
        assert!(!run(&s, &handler, memex_core::EXTRACT_JOB, "w"), "it holds back until due");
        s.engine().clock().advance(Duration::from_secs(600));
        assert!(run(&s, &handler, memex_core::EXTRACT_JOB, "w"));
        assert_eq!(s.extraction_status(&c, &v, &source).unwrap().state, memex_core::ExtractionState::Done);
    }

    fn held_job(s: &Server<Fakes>, v: &VaultId, id: &ObjectId) -> JobRow {
        use memex_types::{RowKey, Table};
        match s
            .engine()
            .store()
            .snapshot(v)
            .unwrap()
            .get(&RowKey {
                table: Table::Job,
                id: id.clone(),
            })
            .unwrap()
        {
            Some(Row::Job(j)) => j,
            other => panic!("{other:?}"),
        }
    }

    /// Whether the page `Derives` writes for this job is there.
    fn derived(s: &Server<Fakes>, v: &VaultId, job: &ObjectId) -> bool {
        let key = memex_types::RowKey {
            table: memex_types::Table::Page,
            id: ObjectId(format!("d-{job}")),
        };
        s.engine().store().snapshot(v).unwrap().get(&key).unwrap().is_some()
    }

    fn the_job(s: &Server<Fakes>, v: &VaultId) -> JobRow {
        s.engine().store().snapshot(v).unwrap().jobs(None, None, None, 10).unwrap().remove(0)
    }

    /// Waits, in real time, for what a keeper does on its own thread.
    fn eventually(what: &str, done: impl Fn() -> bool) {
        for _ in 0..600 {
            if done() {
                return;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        panic!("this never happened: {what}");
    }

    /// A handler that does `during` while it holds the job, and then says it's finished, so anything written must be stopped by the fence.
    struct Scripted<F>(F, AtomicU32);

    impl<F: Fn(&VaultId, &JobRow, &JobHold) + Send + Sync + 'static> JobHandler for Scripted<F> {
        fn run(&self, vault: &VaultId, job: &JobRow, hold: &JobHold) -> Result<Vec<Mutation>, JobFailure> {
            self.1.fetch_add(1, Ordering::SeqCst);
            (self.0)(vault, job, hold);
            Derives(AtomicU32::new(0)).run(vault, job, hold)
        }
    }

    fn scripted<F: Fn(&VaultId, &JobRow, &JobHold) + Send + Sync + 'static>(during: F) -> Arc<Scripted<F>> {
        Arc::new(Scripted(during, AtomicU32::new(0)))
    }

    #[test]
    fn a_job_running_for_three_leases_finishes_in_one_attempt_and_nobody_else_can_claim_it() {
        let (s, v) = setup(1);
        let renewals = Arc::new(AtomicU32::new(0));
        let handler = scripted({
            let (s, renewals) = (s.clone(), renewals.clone());
            move |vault, job, hold| {
                let clock = || s.engine().clock();
                let start = job.attempts[0].started_at_ms;
                let mut last_until = job.lease_until_ms.unwrap();
                // 200 s in steps of one renewal: over three leases.
                for _ in 0..10 {
                    clock().advance(Duration::from_millis(memex_core::jobs::RENEW_EVERY_MS as u64));
                    eventually("the lease moves on", || {
                        held_job(&s, vault, &job.header.id).lease_until_ms.unwrap() > last_until
                    });
                    renewals.fetch_add(1, Ordering::SeqCst);
                    let now = held_job(&s, vault, &job.header.id);
                    last_until = now.lease_until_ms.unwrap();
                    assert!(
                        s.claim_job(vault, "rendition", "second").unwrap().is_none(),
                        "a second worker can't claim a job being renewed"
                    );
                    assert_eq!((now.state, now.attempts.len()), (JobState::Running, 1));
                    assert!(!hold.lost());
                }
                use memex_core::Clock as _;
                assert!(clock().now_ms() - start >= 3 * memex_core::jobs::LEASE_MS);
            }
        });
        assert!(run(&s, &handler, "rendition", "first"));
        assert_eq!(renewals.load(Ordering::SeqCst), 10);
        assert_eq!(handler.1.load(Ordering::SeqCst), 1);
        let job = the_job(&s, &v);
        assert_eq!((job.state, job.attempts.len()), (JobState::Done, 1));
        assert!(derived(&s, &v, &job.header.id));
    }

    #[test]
    fn a_renewal_is_never_longer_than_one_lease() {
        let (s, _) = setup(1);
        let handler = scripted({
            let s = s.clone();
            move |vault, job, _| {
                use memex_core::Clock as _;
                for _ in 0..3 {
                    s.engine().clock().advance(Duration::from_millis(memex_core::jobs::RENEW_EVERY_MS as u64));
                    let before = s.engine().clock().now_ms();
                    eventually("the lease moves on", || {
                        held_job(&s, vault, &job.header.id).lease_until_ms == Some(before + memex_core::jobs::LEASE_MS)
                    });
                }
            }
        });
        assert!(run(&s, &handler, "rendition", "first"));
    }

    #[test]
    fn a_short_job_makes_no_renewal() {
        let (s, v) = setup(1);
        let before = s.engine().store().snapshot(&v).unwrap().head().unwrap();
        assert!(run(&s, &Arc::new(Derives(AtomicU32::new(0))), "rendition", "first"));
        // Its claim and its completion.
        assert_eq!(s.engine().store().snapshot(&v).unwrap().head().unwrap(), before.map(|h| h + 2));
    }

    #[test]
    fn a_job_cancelled_while_it_runs_is_told_at_the_next_renewal_and_writes_nothing() {
        let (s, v, c) = setup_with_caller(1);
        let handler = scripted({
            let s = s.clone();
            move |vault, job, hold| {
                s.cancel_job(&c, vault, &job.header.id).unwrap();
                assert!(!hold.lost());
                s.engine().clock().advance(Duration::from_millis(memex_core::jobs::RENEW_EVERY_MS as u64));
                eventually("the handler hears", || hold.lost());
            }
        });
        assert!(run(&s, &handler, "rendition", "first"));
        assert_eq!(count(&s, &v, JobState::Cancelled), 1);
        assert!(!derived(&s, &v, &the_job(&s, &v).header.id), "its result was refused");
    }

    #[test]
    fn a_job_taken_while_it_runs_is_told_at_the_next_renewal_and_writes_nothing() {
        let (s, v) = setup(1);
        let taken = Arc::new(std::sync::Mutex::new(None));
        let handler = scripted({
            let (s, taken) = (s.clone(), taken.clone());
            move |vault, _, hold| {
                // A stall long enough to lose the lease, as a stopped machine would give. The keeper may get in first once, so it goes on until the other worker has it.
                let second = loop {
                    s.engine().clock().advance(Duration::from_millis(memex_core::jobs::LEASE_MS as u64));
                    if let Some(j) = s.claim_job(vault, "rendition", "second").unwrap() {
                        break j;
                    }
                };
                s.engine().clock().advance(Duration::from_millis(memex_core::jobs::RENEW_EVERY_MS as u64));
                eventually("the handler hears", || hold.lost());
                *taken.lock().unwrap() = Some(second);
            }
        });
        assert!(run(&s, &handler, "rendition", "first"));
        assert!(!derived(&s, &v, &the_job(&s, &v).header.id), "its result was refused");
        let second = taken.lock().unwrap().take().unwrap();
        let now = held_job(&s, &v, &second.header.id);
        assert_eq!(
            (now.state, now.lease_owner.as_deref(), now.lease_until_ms),
            (JobState::Running, Some("second"), second.lease_until_ms),
            "and it left the new holder alone"
        );
        assert!(s.complete_job(&v, &second, vec![]).unwrap());
    }

    #[test]
    fn a_store_that_fails_a_renewal_does_not_stop_the_handler() {
        let (s, v) = setup(1);
        let handler = scripted({
            let s = s.clone();
            move |vault, job, hold| {
                s.engine().store().fail_next_commit(memex_core::StoreError::Backend("away".into()));
                let first = job.lease_until_ms.unwrap();
                s.engine().clock().advance(Duration::from_millis(memex_core::jobs::RENEW_EVERY_MS as u64));
                // The tries after the failed one find the store back, and the job was never lost.
                eventually("the lease moves on", || {
                    s.engine().clock().advance(Duration::from_millis(RENEW_RETRY_MS as u64));
                    held_job(&s, vault, &job.header.id).lease_until_ms.unwrap() > first
                });
                assert!(!hold.lost());
            }
        });
        assert!(run(&s, &handler, "rendition", "first"));
        assert!(derived(&s, &v, &the_job(&s, &v).header.id));
    }

    /// A handler that hangs on the jobs `hangs` picks until it's released, and then answers with a page of its own, which must never be written. Other jobs it finishes at once.
    struct Hangs {
        hangs: Box<dyn Fn(&JobRow) -> bool + Send + Sync>,
        release: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
        started: AtomicU32,
        returned: AtomicU32,
        told_lost: AtomicBool,
    }

    fn hangs(pick: impl Fn(&JobRow) -> bool + Send + Sync + 'static) -> (Arc<Hangs>, std::sync::mpsc::Sender<()>) {
        let (release, wait) = channel();
        let h = Hangs {
            hangs: Box::new(pick),
            release: std::sync::Mutex::new(wait),
            started: AtomicU32::new(0),
            returned: AtomicU32::new(0),
            told_lost: AtomicBool::new(false),
        };
        (Arc::new(h), release)
    }

    fn late_page(job: &ObjectId) -> ObjectId {
        ObjectId(format!("late-{job}"))
    }

    impl JobHandler for Hangs {
        fn run(&self, vault: &VaultId, job: &JobRow, hold: &JobHold) -> Result<Vec<Mutation>, JobFailure> {
            if !(self.hangs)(job) {
                return Derives(AtomicU32::new(0)).run(vault, job, hold);
            }
            self.started.fetch_add(1, Ordering::SeqCst);
            let _ = self.release.lock().unwrap().recv();
            self.told_lost.store(hold.lost(), Ordering::SeqCst);
            self.returned.fetch_add(1, Ordering::SeqCst);
            let header = Header {
                id: late_page(&job.header.id),
                creator: "w".into(),
                status: Status::Active,
                version: 1,
                acl_id: None,
            };
            Ok(vec![Mutation::Put(Row::Page(PageRow {
                header,
                title: "late".into(),
                body: "late".into(),
                updated_at_ms: 0,
                slug: String::new(),
                redirects: vec![],
            }))])
        }
    }

    fn has_page(s: &Server<Fakes>, v: &VaultId, id: ObjectId) -> bool {
        let key = memex_types::RowKey {
            table: memex_types::Table::Page,
            id,
        };
        s.engine().store().snapshot(v).unwrap().get(&key).unwrap().is_some()
    }

    #[test]
    fn a_handler_that_never_returns_is_failed_at_its_ceiling_and_its_late_result_writes_nothing() {
        let (s, v) = setup(1);
        let ceiling = memex_core::JobCeilings::default().for_kind("rendition");
        let (handler, release) = hangs(|_| true);
        let first = std::thread::spawn({
            let (s, handler) = (s.clone(), handler.clone());
            move || run_one(&s, &handler, "rendition", "first", ceiling)
        });
        eventually("the handler starts", || handler.started.load(Ordering::SeqCst) == 1);
        let job = the_job(&s, &v);
        // A second short of the ceiling, it's still running and still held.
        s.engine().clock().advance(ceiling - Duration::from_secs(1));
        eventually("the lease moves on", || held_job(&s, &v, &job.header.id).lease_until_ms > job.lease_until_ms);
        assert!(!first.is_finished());
        assert_eq!(held_job(&s, &v, &job.header.id).state, JobState::Running);
        assert!(s.claim_job(&v, "rendition", "second").unwrap().is_none());
        // At the ceiling the worker gives up on it, while the handler still hangs.
        s.engine().clock().advance(Duration::from_secs(1));
        eventually("the worker gives up", || first.is_finished());
        assert!(first.join().unwrap());
        assert_eq!(handler.returned.load(Ordering::SeqCst), 0, "the handler is still hanging");
        assert!(abandoned_handlers() >= 1);
        let failed = held_job(&s, &v, &job.header.id);
        assert_eq!(
            (failed.state, failed.attempts.len(), failed.lease_owner.as_deref()),
            (JobState::Queued, 1, None)
        );
        let reason = failed.attempts[0].error.clone().unwrap();
        assert_eq!(reason, past_ceiling_message("rendition", ceiling));
        assert!(
            reason.contains("ceiling of 14400 s for rendition jobs, set by MEMEX_RENDITION_CEILING_S"),
            "{reason}"
        );
        // Item 2: a second worker claims it after its delay, and finishes it.
        assert!(s.claim_job(&v, "rendition", "second").unwrap().is_none(), "it waits out its delay first");
        s.engine().clock().advance(Duration::from_secs(60));
        assert!(run(&s, &Arc::new(Derives(AtomicU32::new(0))), "rendition", "second"));
        let done = held_job(&s, &v, &job.header.id);
        assert_eq!((done.state, done.attempts.len()), (JobState::Done, 2));
        assert!(derived(&s, &v, &job.header.id));
        // The hung handler returns at last, told its job was lost, and what it returns writes nothing.
        let head = s.engine().store().snapshot(&v).unwrap().head().unwrap();
        release.send(()).unwrap();
        eventually("the handler returns", || handler.returned.load(Ordering::SeqCst) == 1);
        assert!(handler.told_lost.load(Ordering::SeqCst));
        std::thread::sleep(KEEPER_POLL * 2);
        assert_eq!(s.engine().store().snapshot(&v).unwrap().head().unwrap(), head, "nothing was written after it");
        assert!(!has_page(&s, &v, late_page(&job.header.id)));
        assert_eq!(held_job(&s, &v, &job.header.id), done);
    }

    #[test]
    fn a_hung_handler_does_not_stop_other_jobs_of_its_kind() {
        let (s, v) = setup(3);
        let ceiling = Duration::from_secs(600);
        // The first job it sees hangs on its first attempt. Every other one finishes.
        let picked = Arc::new(std::sync::Mutex::new(None::<ObjectId>));
        let (handler, release) = hangs({
            let picked = picked.clone();
            move |job| {
                let mut p = picked.lock().unwrap();
                let id = p.get_or_insert_with(|| job.header.id.clone());
                *id == job.header.id && job.attempts.len() == 1
            }
        });
        let stop = Arc::new(AtomicBool::new(false));
        // One worker, as the embedding pool has.
        let threads = start(
            s.clone(),
            vec![Pool {
                kind: "rendition".into(),
                workers: 1,
                handler: handler.clone(),
                ceiling,
            }],
            stop.clone(),
        );
        eventually("the handler hangs", || handler.started.load(Ordering::SeqCst) == 1);
        assert_eq!(count(&s, &v, JobState::Queued), 2, "with its one worker hung, the others wait for the ceiling");
        s.engine().clock().advance(ceiling);
        eventually("the other two finish", || count(&s, &v, JobState::Done) == 2);
        // The worker it had is free again, and does the retry too, while the hung thread still hangs.
        s.engine().clock().advance(Duration::from_secs(60));
        eventually("all three finish", || count(&s, &v, JobState::Done) == 3);
        assert_eq!(handler.returned.load(Ordering::SeqCst), 0);
        let id = picked.lock().unwrap().clone().unwrap();
        let retried = held_job(&s, &v, &id);
        assert_eq!(retried.attempts.len(), 2);
        assert!(retried.attempts[0].error.as_deref().unwrap().contains("ceiling"));
        release.send(()).unwrap();
        eventually("the handler returns", || handler.returned.load(Ordering::SeqCst) == 1);
        stop.store(true, Ordering::SeqCst);
        for t in threads {
            t.join().unwrap();
        }
        assert!(!has_page(&s, &v, late_page(&id)));
    }

    #[test]
    fn a_panic_ends_the_keeper_too() {
        let (s, v) = setup(1);
        assert!(run(&s, &Arc::new(Panics), "rendition", "first"));
        let head = s.engine().store().snapshot(&v).unwrap().head().unwrap();
        s.engine().clock().advance(Duration::from_secs(3600));
        std::thread::sleep(KEEPER_POLL * 3);
        assert_eq!(
            s.engine().store().snapshot(&v).unwrap().head().unwrap(),
            head,
            "nothing renews a job no handler is running"
        );
    }

    #[test]
    fn a_source_with_a_missing_blob_fails_once_and_says_so() {
        use memex_core::BlobStore as _;
        let (s, v) = setup(1);
        let job = the_job(&s, &v);
        let source = ObjectId(job.input["source_id"].as_str().unwrap().into());
        let row = match s
            .engine()
            .store()
            .snapshot(&v)
            .unwrap()
            .get(&memex_types::RowKey {
                table: memex_types::Table::Source,
                id: source,
            })
            .unwrap()
        {
            Some(Row::Source(row)) => row,
            other => panic!("{other:?}"),
        };
        s.blobs().delete(&v, &memex_types::BlobHash::parse(&row.sha256).unwrap()).unwrap();
        let handler = Arc::new(crate::rendition_job::RenditionHandler {
            server: s.clone(),
            worker: crate::rendition_job::WorkerCommand {
                program: "/bin/false".into(),
                args: vec![],
                timeout: Duration::from_secs(5),
            },
            ocr: None,
        });
        assert!(run(&s, &handler, "rendition", "w"));
        let job = the_job(&s, &v);
        assert_eq!((job.state, job.attempts.len()), (JobState::Dead, 1));
        assert!(job.attempts[0].error.as_deref().unwrap().contains("missing"), "{:?}", job.attempts);
    }

    #[test]
    fn no_work_returns_false() {
        let (s, _) = setup(0);
        assert!(!run(&s, &Arc::new(Panics), "rendition", "w"));
    }
}
