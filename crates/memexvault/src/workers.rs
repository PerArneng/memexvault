//! The worker pool: threads separate from request handling, with a limit per job kind.
// Workers wait on wall time between polls, which is this edge's job.
#![allow(clippy::disallowed_methods)]
use memex_core::{Server, ServerEdges};
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
            })
        });
        let recorded = match outcome {
            Ok(results) => server.complete_job(&vault, &job, results),
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
    }

    fn setup(n: u32) -> (Arc<Server<Fakes>>, memex_types::VaultId) {
        let s = Arc::new(Server::<Fakes>::new(
            MemRegistry::new(),
            MemStore::new(),
            MemBlobStore::new(ManualClock::starting_at(0)),
            ManualClock::starting_at(1000),
            SeededRandom::new(5),
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
        (s, v)
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

    #[test]
    fn no_work_returns_false() {
        let (s, _) = setup(0);
        assert!(!run_one(s.as_ref(), &Panics, "rendition", "w"));
    }
}
