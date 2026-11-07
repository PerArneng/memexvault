//! Keeps a vault's keyword index up to date with its log, and rebuilds it when it can't be trusted.
use crate::lexical::{CommitPolicy, LexicalIndex, Touched, assertion_ops, segment_ops};
use crate::{Clock, PAGE_LIMIT, Snapshot, Store, StoreError, UpcastError};
use memex_types::{Event, Generation, IndexHealth, IndexOp, LexicalError, ObjectId, Row, RowKey, Table, VaultId, VectorError, entity_of_subject};
use std::collections::{BTreeSet, HashMap};
use std::fmt;
use std::time::Duration;

#[derive(Debug)]
pub enum ProjectError {
    Store(StoreError),
    Index(LexicalError),
    Upcast(UpcastError),
    Vector(VectorError),
    /// Embed jobs couldn't be queued.
    Enqueue(String),
}

impl fmt::Display for ProjectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(e) => write!(f, "{e}"),
            Self::Index(e) => write!(f, "{e}"),
            Self::Upcast(e) => write!(f, "{e}"),
            Self::Vector(e) => write!(f, "{e}"),
            Self::Enqueue(m) => write!(f, "can't queue embedding: {m}"),
        }
    }
}

impl std::error::Error for ProjectError {}

impl From<StoreError> for ProjectError {
    fn from(e: StoreError) -> Self {
        Self::Store(e)
    }
}

impl From<VectorError> for ProjectError {
    fn from(e: VectorError) -> Self {
        Self::Vector(e)
    }
}

impl From<LexicalError> for ProjectError {
    fn from(e: LexicalError) -> Self {
        Self::Index(e)
    }
}

/// Whether a step did anything, so the caller knows whether to wait.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepOutcome {
    Idle,
    Progress,
}

/// How far a generation has got: events applied, what's written since its commit, and when that commit was.
#[derive(Debug, Clone, Copy)]
struct Applied {
    upto: Option<u64>,
    committed: Option<u64>,
    pending: usize,
    last_commit: Duration,
}

impl Applied {
    fn at(watermark: Option<u64>, now: Duration) -> Self {
        Self {
            upto: watermark,
            committed: watermark,
            pending: 0,
            last_commit: now,
        }
    }
}

#[derive(Debug, Clone)]
enum Build {
    /// Nothing done yet.
    Start,
    /// Pages through a table. A row changed after the start is picked up by this page or by catching up from `from`, so no long-lived snapshot is needed.
    Scan { from: Option<u64>, table: Table, after: Option<ObjectId> },
    /// Applies what happened since the scan began, then flips.
    CatchUp(Applied),
}

#[derive(Debug, Clone, Default)]
struct VaultState {
    checked: bool,
    live: Option<Applied>,
    build: Option<Build>,
}

/// One projector per server. Its state is only progress: all of it comes back from the index and the log after a restart.
#[derive(Debug, Default)]
pub struct LexicalProjector {
    policy: CommitPolicy,
    vaults: HashMap<VaultId, VaultState>,
}

const SCAN_ORDER: [Table; 5] = [Table::Segment, Table::Assertion, Table::Entity, Table::Page, Table::Block];

fn ids_of(set: impl IntoIterator<Item = ObjectId>) -> Vec<ObjectId> {
    set.into_iter().collect()
}

/// Everything these events change in the index.
pub(crate) fn ops_for_events<S: Snapshot>(snap: &S, events: &[Event]) -> Result<Vec<IndexOp>, ProjectError> {
    let mut touched = Touched::default();
    for e in events {
        touched.add_event(e).map_err(ProjectError::Upcast)?;
    }
    for source in std::mem::take(&mut touched.sources) {
        // A source's segments and claims carry its trust tier, so all of them are written again. This is rare, and bounded by the source's size.
        for r in snap.renditions_of(&source)? {
            let mut after = None;
            loop {
                let page = snap.segments_of(&r.header.id, after.as_ref(), PAGE_LIMIT)?;
                let Some(last) = page.last() else { break };
                after = Some(last.header.id.clone());
                let ids: Vec<memex_types::ObjectId> = page.into_iter().map(|s| s.header.id).collect();
                let citing: Vec<ObjectId> = snap.evidence_citing(&ids)?.into_iter().map(|e| e.assertion_id).collect();
                touched.entities.extend(entities_behind(snap, &citing)?);
                touched.assertions.extend(citing);
                touched.segments.extend(ids);
            }
        }
    }
    for rendition in std::mem::take(&mut touched.renditions) {
        let mut after = None;
        loop {
            let page = snap.segments_of(&rendition, after.as_ref(), PAGE_LIMIT)?;
            let Some(last) = page.last() else { break };
            after = Some(last.header.id.clone());
            touched.segments.extend(page.into_iter().map(|s| s.header.id));
        }
    }
    for name in std::mem::take(&mut touched.entity_types) {
        // An entity's document depends on its type's annotations. Rare, as with a changed source, and a brand-new type has nothing to write.
        let mut after = None;
        loop {
            let page = snap.entities_of_type(&name, after.as_ref(), PAGE_LIMIT)?;
            let Some(last) = page.last() else { break };
            after = Some(last.header.id.clone());
            touched.entities.extend(page.into_iter().map(|e| e.header.id));
        }
    }
    let mut ops = vec![];
    for chunk in ids_of(touched.segments).chunks(PAGE_LIMIT) {
        ops.extend(segment_ops(snap, chunk)?);
    }
    for chunk in ids_of(touched.assertions).chunks(PAGE_LIMIT) {
        ops.extend(assertion_ops(snap, chunk)?);
    }
    for chunk in ids_of(touched.entities).chunks(PAGE_LIMIT) {
        ops.extend(crate::entity::entity_ops(snap, chunk)?);
    }
    for chunk in ids_of(touched.pages).chunks(PAGE_LIMIT) {
        ops.extend(crate::page::page_ops(snap, chunk)?);
    }
    for chunk in ids_of(touched.blocks).chunks(PAGE_LIMIT) {
        ops.extend(crate::page::block_ops(snap, chunk)?);
    }
    Ok(ops)
}

/// The current entities these claims are about, following merges. Two store calls, and one more per hop of a merge.
fn entities_behind<S: Snapshot>(snap: &S, claims: &[ObjectId]) -> Result<Vec<ObjectId>, ProjectError> {
    let keys: Vec<RowKey> = claims
        .iter()
        .map(|id| RowKey {
            table: Table::Assertion,
            id: id.clone(),
        })
        .collect();
    if keys.is_empty() {
        return Ok(vec![]);
    }
    let mut at: BTreeSet<ObjectId> = snap
        .get_many(&keys)?
        .into_iter()
        .filter_map(|r| match r {
            Some(Row::Assertion(a)) => entity_of_subject(&a.subject),
            _ => None,
        })
        .collect();
    let mut found = vec![];
    // Bounded, as a merge chain is, so a bad row can't hold the projector.
    for _ in 0..64 {
        if at.is_empty() {
            break;
        }
        let keys: Vec<RowKey> = at
            .iter()
            .map(|id| RowKey {
                table: Table::Entity,
                id: id.clone(),
            })
            .collect();
        let mut next = BTreeSet::new();
        for row in snap.get_many(&keys)?.into_iter().flatten() {
            if let Row::Entity(e) = row {
                match e.redirect_to {
                    Some(to) => {
                        next.insert(to);
                    }
                    None => found.push(e.header.id),
                }
            }
        }
        at = next;
    }
    Ok(found)
}

impl LexicalProjector {
    pub fn new(policy: CommitPolicy) -> Self {
        Self {
            policy,
            vaults: HashMap::new(),
        }
    }

    /// Asks for a new generation. The current one keeps serving until it's ready.
    pub fn request_rebuild(&mut self, vault: &VaultId) {
        self.vaults.entry(vault.clone()).or_default().build = Some(Build::Start);
    }

    /// Forgets this vault's progress, for after it's deleted.
    pub fn forget(&mut self, vault: &VaultId) {
        self.vaults.remove(vault);
    }

    /// Does a bounded piece of work. On an error, the next step checks the index again from its durable state.
    pub fn step<S: Store, L: LexicalIndex, C: Clock>(&mut self, store: &S, index: &L, clock: &C, vault: &VaultId) -> Result<StepOutcome, ProjectError> {
        let mut state = self.vaults.remove(vault).unwrap_or_default();
        let result = self.advance(store, index, clock, vault, &mut state);
        if result.is_err() {
            // An unfinished build starts again, since a failed write may have lost documents.
            state = VaultState {
                checked: false,
                live: None,
                build: state.build.map(|_| Build::Start),
            };
        }
        self.vaults.insert(vault.clone(), state);
        result
    }

    fn advance<S: Store, L: LexicalIndex, C: Clock>(
        &self,
        store: &S,
        index: &L,
        clock: &C,
        vault: &VaultId,
        state: &mut VaultState,
    ) -> Result<StepOutcome, ProjectError> {
        let snap = store.snapshot(vault)?;
        let head = snap.head()?;
        let now = clock.monotonic();
        if !state.checked {
            self.check(index, vault, head, state, now)?;
            return Ok(StepOutcome::Progress);
        }
        // A search can find the damage after the startup check did not.
        if !matches!(index.health(vault, false)?, IndexHealth::Ready { .. }) && state.live.is_some() {
            state.checked = false;
            state.live = None;
            return Ok(StepOutcome::Progress);
        }
        let mut outcome = StepOutcome::Idle;
        if let Some(live) = state.live.as_mut() {
            outcome = self.follow(&snap, index, vault, Generation::Live, live, head, now)?;
        }
        if let Some(build) = state.build.take() {
            state.build = self.build_step(&snap, index, vault, build, head, now, &mut state.live)?;
            outcome = StepOutcome::Progress;
        }
        Ok(outcome)
    }

    fn check<L: LexicalIndex>(&self, index: &L, vault: &VaultId, head: Option<u64>, state: &mut VaultState, now: Duration) -> Result<(), ProjectError> {
        match index.health(vault, true)? {
            IndexHealth::Ready { watermark } if watermark <= head => {
                state.live = Some(Applied::at(watermark, now));
            }
            IndexHealth::Ready { watermark } => {
                tracing::warn!(vault = %vault, ?watermark, ?head, "the keyword index is ahead of the log, so it's being rebuilt");
                state.live = None;
                self.start_build_if_none(state);
            }
            IndexHealth::Missing | IndexHealth::Outdated => {
                state.live = None;
                self.start_build_if_none(state);
            }
            IndexHealth::Corrupt(detail) => {
                tracing::warn!(vault = %vault, %detail, "the keyword index is damaged, so it's being rebuilt");
                state.live = None;
                self.start_build_if_none(state);
            }
        }
        state.checked = true;
        Ok(())
    }

    fn start_build_if_none(&self, state: &mut VaultState) {
        state.build.get_or_insert(Build::Start);
    }

    /// Applies one page of events to a generation, and commits when the policy says so.
    #[allow(clippy::too_many_arguments)]
    fn follow<S: Snapshot, L: LexicalIndex>(
        &self,
        snap: &S,
        index: &L,
        vault: &VaultId,
        generation: Generation,
        applied: &mut Applied,
        head: Option<u64>,
        now: Duration,
    ) -> Result<StepOutcome, ProjectError> {
        let mut outcome = StepOutcome::Idle;
        if head > applied.upto {
            let events = snap.events(applied.upto)?;
            if let Some(last) = events.last() {
                let ops = ops_for_events(snap, &events)?;
                if !ops.is_empty() {
                    index.write(vault, generation, &ops)?;
                    applied.pending += ops.len();
                }
                applied.upto = Some(last.seq);
                outcome = StepOutcome::Progress;
            }
        }
        if self
            .policy
            .should_commit(applied.pending, applied.upto != applied.committed, now.saturating_sub(applied.last_commit))
        {
            index.commit(vault, generation, applied.upto)?;
            applied.committed = applied.upto;
            applied.pending = 0;
            applied.last_commit = now;
            outcome = StepOutcome::Progress;
        }
        Ok(outcome)
    }

    #[allow(clippy::too_many_arguments)]
    fn build_step<S: Snapshot, L: LexicalIndex>(
        &self,
        snap: &S,
        index: &L,
        vault: &VaultId,
        build: Build,
        head: Option<u64>,
        now: Duration,
        live: &mut Option<Applied>,
    ) -> Result<Option<Build>, ProjectError> {
        match build {
            // Everything after this point is covered by catching up.
            Build::Start => {
                index.begin_build(vault)?;
                Ok(Some(Build::Scan {
                    from: head,
                    table: SCAN_ORDER[0],
                    after: None,
                }))
            }
            Build::Scan { from, table, after } => {
                let page = snap.rows(table, after.as_ref())?;
                let Some(last) = page.last().map(|r| r.header().id.clone()) else {
                    let next = SCAN_ORDER.iter().position(|t| *t == table).and_then(|i| SCAN_ORDER.get(i + 1));
                    return Ok(Some(match next {
                        Some(next) => Build::Scan {
                            from,
                            table: *next,
                            after: None,
                        },
                        None => {
                            index.commit(vault, Generation::Building, from)?;
                            Build::CatchUp(Applied::at(from, now))
                        }
                    }));
                };
                let ids: Vec<ObjectId> = page.iter().map(|r| r.header().id.clone()).collect();
                let ops = match page.first() {
                    Some(Row::Assertion(_)) => assertion_ops(snap, &ids)?,
                    Some(Row::Entity(_)) => crate::entity::entity_ops(snap, &ids)?,
                    Some(Row::Page(_)) => crate::page::page_ops(snap, &ids)?,
                    Some(Row::Block(_)) => crate::page::block_ops(snap, &ids)?,
                    _ => segment_ops(snap, &ids)?,
                };
                index.write(vault, Generation::Building, &ops)?;
                Ok(Some(Build::Scan {
                    from,
                    table,
                    after: Some(last),
                }))
            }
            Build::CatchUp(mut applied) => {
                let mut caught_up = true;
                if head > applied.upto {
                    let events = snap.events(applied.upto)?;
                    caught_up = events.len() < PAGE_LIMIT;
                    if let Some(last) = events.last() {
                        let ops = ops_for_events(snap, &events)?;
                        if !ops.is_empty() {
                            index.write(vault, Generation::Building, &ops)?;
                        }
                        applied.upto = Some(last.seq);
                    }
                }
                if !caught_up {
                    return Ok(Some(Build::CatchUp(applied)));
                }
                index.commit(vault, Generation::Building, applied.upto)?;
                index.flip(vault)?;
                *live = Some(Applied::at(applied.upto, now));
                tracing::info!(vault = %vault, watermark = ?applied.upto, "keyword index generation is live");
                Ok(None)
            }
        }
    }
}
