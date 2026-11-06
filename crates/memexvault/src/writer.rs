//! Group commit: one thread per vault takes what's waiting, commits it in one transaction, and answers each caller only after that commit.
use memex_core::{ApplyError, BatchLimits, Clock, Engine, Receipt, Store, StoreError, WriterLease, plan_batch};
use memex_types::EventDraft;
use std::collections::VecDeque;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::JoinHandle;

struct Job {
    draft: EventDraft,
    size: usize,
    reply: Sender<Result<Receipt, ApplyError>>,
}

#[derive(Clone)]
pub struct WriterHandle {
    tx: Sender<Job>,
}

impl WriterHandle {
    /// Blocks until the event is durable or refused.
    pub fn submit(&self, draft: EventDraft) -> Result<Receipt, ApplyError> {
        let (reply, answer) = channel();
        let size = draft.payload.to_string().len();
        let gone = || ApplyError::Store(StoreError::Backend("writer stopped".into()));
        self.tx.send(Job { draft, size, reply }).map_err(|_| gone())?;
        answer.recv().map_err(|_| gone())?
    }
}

/// Starts the vault's writer. It stops when every handle is dropped.
pub fn start_writer<S, C>(engine: Engine<S, C>, lease: WriterLease, limits: BatchLimits) -> (WriterHandle, JoinHandle<Engine<S, C>>)
where
    S: Store + Send + 'static,
    C: Clock + Send + 'static,
{
    let (tx, rx) = channel();
    let handle = std::thread::spawn(move || {
        run(&engine, &lease, limits, &rx);
        engine
    });
    (WriterHandle { tx }, handle)
}

fn run<S: Store, C: Clock>(engine: &Engine<S, C>, lease: &WriterLease, limits: BatchLimits, rx: &Receiver<Job>) {
    let mut waiting: VecDeque<Job> = VecDeque::new();
    loop {
        if waiting.is_empty() {
            match rx.recv() {
                Ok(job) => waiting.push_back(job),
                Err(_) => return,
            }
        }
        waiting.extend(rx.try_iter());
        let sizes: Vec<usize> = waiting.iter().take(limits.max_events.max(1)).map(|j| j.size).collect();
        let take = plan_batch(&sizes, limits);
        let jobs: Vec<Job> = waiting.drain(..take).collect();
        let (drafts, replies): (Vec<EventDraft>, Vec<Sender<_>>) = jobs.into_iter().map(|j| (j.draft, j.reply)).unzip();
        for (result, reply) in engine.apply_batch(lease, drafts).into_iter().zip(replies) {
            // A caller that went away no longer needs its answer.
            let _ = reply.send(result);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use memex_core::testing::{ManualClock, MemStore};
    use memex_core::{Snapshot, StoreError};
    use memex_types::VaultId;
    use serde_json::json;
    use std::sync::atomic::Ordering;

    fn draft(id: &str) -> EventDraft {
        EventDraft {
            actor: "t".into(),
            kind: "page.created".into(),
            schema_version: 1,
            payload: json!({"id": id, "title": "T", "body": "B"}),
        }
    }

    const LIMITS: BatchLimits = BatchLimits {
        max_events: 64,
        max_bytes: 1 << 20,
    };

    #[test]
    fn concurrent_callers_are_grouped_and_all_answered() {
        let store = MemStore::new();
        let lease = store.open_vault(&VaultId("v".into())).unwrap();
        let (handle, join) = start_writer(Engine::new(store, ManualClock::starting_at(0)), lease, LIMITS);
        let threads: Vec<_> = (0..8)
            .map(|t| {
                let h = handle.clone();
                std::thread::spawn(move || (0..500).map(|i| h.submit(draft(&format!("p{t}-{i}"))).unwrap().seq).collect::<Vec<u64>>())
            })
            .collect();
        let mut seqs: Vec<u64> = threads.into_iter().flat_map(|t| t.join().unwrap()).collect();
        drop(handle);
        let engine = join.join().unwrap();
        seqs.sort_unstable();
        assert_eq!(seqs, (0..4000).collect::<Vec<u64>>());
        assert_eq!(engine.store().snapshot(&VaultId("v".into())).unwrap().head().unwrap(), Some(3999));
        assert!(engine.store().counts().commits.load(Ordering::Relaxed) <= 4000);
    }

    #[test]
    fn a_refused_draft_is_answered_with_its_own_error() {
        let store = MemStore::new();
        let lease = store.open_vault(&VaultId("v".into())).unwrap();
        let (handle, join) = start_writer(Engine::new(store, ManualClock::starting_at(0)), lease, LIMITS);
        handle.submit(draft("p")).unwrap();
        assert!(matches!(handle.submit(draft("p")), Err(ApplyError::Reduce(_))));
        assert_eq!(handle.submit(draft("q")).unwrap().seq, 1);
        drop(handle);
        join.join().unwrap();
    }

    #[test]
    fn a_failed_commit_reaches_the_caller() {
        let store = MemStore::new();
        let lease = store.open_vault(&VaultId("v".into())).unwrap();
        store.fail_next_commit(StoreError::Backend("x".into()));
        let (handle, join) = start_writer(Engine::new(store, ManualClock::starting_at(0)), lease, LIMITS);
        assert!(matches!(handle.submit(draft("p")), Err(ApplyError::Store(StoreError::Backend(_)))));
        assert_eq!(handle.submit(draft("p")).unwrap().seq, 0);
        drop(handle);
        join.join().unwrap();
    }
}
