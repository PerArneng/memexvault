use crate::{ReduceError, Snapshot, Store, StoreError, UpcastError, WriterLease, reduce, upcast};
use memex_types::Event;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplayError {
    Upcast(UpcastError),
    Reduce { seq: u64, error: ReduceError },
    Store(StoreError),
}

impl fmt::Display for ReplayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Upcast(e) => write!(f, "{e}"),
            Self::Reduce { seq, error } => write!(f, "event {seq}: {error}"),
            Self::Store(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ReplayError {}

impl From<StoreError> for ReplayError {
    fn from(e: StoreError) -> Self {
        Self::Store(e)
    }
}

/// Applies events to a store in order, as the log recorded them. It stops at the first bad event, leaving everything before it applied.
/// The stored event keeps its original version, and only the reducer sees the upcast form.
pub fn replay<S: Store>(events: impl IntoIterator<Item = Event>, target: &S, lease: &WriterLease) -> Result<u64, ReplayError> {
    let mut applied = 0;
    for event in events {
        let current = upcast(&event).map_err(ReplayError::Upcast)?;
        let mutations = {
            let snap = target.snapshot(&lease.vault)?;
            // Checked here too, so a gap in the log is reported as a gap and not as a confusing reducer error.
            let head = snap.head()?;
            if event.seq != head.map_or(0, |h| h + 1) {
                return Err(StoreError::SequenceGap { head, got: event.seq }.into());
            }
            reduce(&snap, &current).map_err(|error| ReplayError::Reduce { seq: event.seq, error })?
        };
        target.commit(lease, &event, &mutations)?;
        applied += 1;
    }
    Ok(applied)
}

/// All of a snapshot's events, paged.
pub fn all_events<S: Snapshot>(snap: &S) -> Result<Vec<Event>, StoreError> {
    let (mut out, mut after) = (vec![], None);
    loop {
        let page = snap.events(after)?;
        let Some(last) = page.last() else { return Ok(out) };
        after = Some(last.seq);
        out.extend(page);
    }
}
