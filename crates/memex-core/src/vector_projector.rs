//! Keeps a vault's vector index up to date with its log, using stored embeddings, and moves it to a new model side by side.
use crate::lexical::{Touched, assertion_ops, segment_ops};
use crate::projector::{ProjectError, StepOutcome, ops_for_events};
use crate::vector::{EmbedRequest, Embedder, VectorIndex, VectorPlan, embed_requests, embedded_by, plan_ops};
use crate::{PAGE_LIMIT, Snapshot, Store};
use memex_types::{DocKind, Event, Generation, IndexOp, ModelInfo, Mutation, ObjectId, Row, Table, VaultId, VectorHealth, VectorOp};
use std::collections::{BTreeSet, HashMap};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    /// Nothing done yet.
    Start,
    /// Pages through a table. A row changed after the start is picked up by this page or by catching up from `from`.
    Scan,
    /// Applies what happened since the scan began, then flips.
    CatchUp,
}

#[derive(Debug, Clone)]
struct Build {
    model: ModelInfo,
    stage: Stage,
    from: Option<u64>,
    upto: Option<u64>,
    table: Table,
    after: Option<ObjectId>,
    /// Objects with no embedding yet. A move to a new model waits for them, so it doesn't lose results when it flips.
    waiting: BTreeSet<ObjectId>,
}

impl Build {
    fn new(model: ModelInfo) -> Self {
        Self {
            model,
            stage: Stage::Start,
            from: None,
            upto: None,
            table: SCAN_ORDER[0],
            after: None,
            waiting: BTreeSet::new(),
        }
    }

    fn note(&mut self, plan: &VectorPlan) {
        for op in &plan.ops {
            match op {
                VectorOp::Upsert(d) => {
                    self.waiting.remove(&d.id);
                }
                VectorOp::Delete(id) => {
                    self.waiting.remove(id);
                }
            }
        }
        self.waiting.extend(plan.missing.iter().map(|m| m.doc.id.clone()));
    }
}

#[derive(Debug, Clone)]
struct Live {
    model: ModelInfo,
    upto: Option<u64>,
}

#[derive(Debug, Clone, Default)]
struct VaultState {
    checked: bool,
    live: Option<Live>,
    build: Option<Build>,
}

const SCAN_ORDER: [Table; 3] = [Table::Segment, Table::Assertion, Table::Entity];

/// One projector per server. Its state is only progress: all of it comes back from the index and the log after a restart.
#[derive(Debug, Default)]
pub struct VectorProjector {
    vaults: HashMap<VaultId, VaultState>,
}

/// Index writes for these events: what `ops_for_events` finds, plus anything a finished embed job has made ready.
fn event_ops<S: Snapshot>(snap: &S, events: &[Event]) -> Result<Vec<IndexOp>, ProjectError> {
    let mut ops = ops_for_events(snap, events)?;
    let mut touched = Touched::default();
    for e in events {
        if e.kind != "state.recorded" {
            continue;
        }
        let Ok(recorded) = serde_json::from_value::<crate::StateRecorded>(crate::upcast(e).map_err(ProjectError::Upcast)?.payload) else {
            continue;
        };
        for m in recorded.mutations {
            if let Mutation::Put(Row::Job(job)) = m {
                for doc in embedded_by(&job) {
                    match doc.kind {
                        DocKind::Segment => touched.segments.insert(doc.id),
                        DocKind::Claim => touched.assertions.insert(doc.id),
                        DocKind::Entity => touched.entities.insert(doc.id),
                    };
                }
            }
        }
    }
    let segments: Vec<ObjectId> = touched.segments.into_iter().collect();
    for chunk in segments.chunks(PAGE_LIMIT) {
        ops.extend(segment_ops(snap, chunk)?);
    }
    let assertions: Vec<ObjectId> = touched.assertions.into_iter().collect();
    for chunk in assertions.chunks(PAGE_LIMIT) {
        ops.extend(assertion_ops(snap, chunk)?);
    }
    let entities: Vec<ObjectId> = touched.entities.into_iter().collect();
    for chunk in entities.chunks(PAGE_LIMIT) {
        ops.extend(crate::entity::entity_ops(snap, chunk)?);
    }
    Ok(ops)
}

impl VectorProjector {
    /// Asks for a new generation of the active model. The current one keeps serving until it's ready.
    pub fn request_rebuild(&mut self, vault: &VaultId, model: ModelInfo) {
        self.vaults.entry(vault.clone()).or_default().build = Some(Build::new(model));
    }

    /// Forgets this vault's progress, for after it's deleted.
    pub fn forget(&mut self, vault: &VaultId) {
        self.vaults.remove(vault);
    }

    /// Does a bounded piece of work. `enqueue` makes sure the embed jobs exist, and runs before the watermark that depends on them moves.
    pub fn step<S, V, E, Q>(&mut self, store: &S, index: &V, embedder: &E, vault: &VaultId, enqueue: &mut Q) -> Result<StepOutcome, ProjectError>
    where
        S: Store,
        V: VectorIndex,
        E: Embedder,
        Q: FnMut(&[EmbedRequest]) -> Result<(), String>,
    {
        let mut state = self.vaults.remove(vault).unwrap_or_default();
        let result = advance(store, index, embedder, vault, &mut state, enqueue);
        if result.is_err() {
            state = VaultState {
                checked: false,
                live: None,
                build: state.build.map(|b| Build::new(b.model)),
            };
        }
        self.vaults.insert(vault.clone(), state);
        result
    }
}

fn advance<S, V, E, Q>(store: &S, index: &V, embedder: &E, vault: &VaultId, state: &mut VaultState, enqueue: &mut Q) -> Result<StepOutcome, ProjectError>
where
    S: Store,
    V: VectorIndex,
    E: Embedder,
    Q: FnMut(&[EmbedRequest]) -> Result<(), String>,
{
    let snap = store.snapshot(vault)?;
    let head = snap.head()?;
    if !state.checked {
        check(index, embedder, vault, head, state)?;
        return Ok(StepOutcome::Progress);
    }
    let mut outcome = StepOutcome::Idle;
    if let Some(live) = state.live.as_mut()
        && head > live.upto
    {
        let events = snap.events(live.upto)?;
        if let Some(last) = events.last() {
            let plan = plan_ops(&snap, index, vault, &live.model.name, event_ops(&snap, &events)?)?;
            send(enqueue, &live.model.name, &plan)?;
            index.apply(vault, Generation::Live, &plan.ops, Some(last.seq))?;
            live.upto = Some(last.seq);
            outcome = StepOutcome::Progress;
        }
    }
    if let Some(build) = state.build.take() {
        let (next, progress) = build_step(&snap, index, vault, build, head, &mut state.live, enqueue)?;
        state.build = next;
        if progress {
            outcome = StepOutcome::Progress;
        }
    }
    Ok(outcome)
}

fn check<V: VectorIndex, E: Embedder>(index: &V, embedder: &E, vault: &VaultId, head: Option<u64>, state: &mut VaultState) -> Result<(), ProjectError> {
    let active = embedder.active();
    match index.health(vault)? {
        VectorHealth::Ready { model, watermark } if watermark <= head => {
            if model != active && state.build.is_none() {
                tracing::info!(vault = %vault, from = %model.name, to = %active.name, "moving the vector index to a new embedding model");
                state.build = Some(Build::new(active));
            }
            state.live = Some(Live { model, upto: watermark });
        }
        VectorHealth::Ready { watermark, .. } => {
            tracing::warn!(vault = %vault, ?watermark, ?head, "the vector index is ahead of the log, so it's being rebuilt");
            state.live = None;
            state.build.get_or_insert_with(|| Build::new(active));
        }
        VectorHealth::Missing => {
            state.live = None;
            state.build.get_or_insert_with(|| Build::new(active));
        }
    }
    state.checked = true;
    Ok(())
}

fn send<Q: FnMut(&[EmbedRequest]) -> Result<(), String>>(enqueue: &mut Q, model: &str, plan: &VectorPlan) -> Result<(), ProjectError> {
    if plan.missing.is_empty() {
        return Ok(());
    }
    enqueue(&embed_requests(model, &plan.missing)).map_err(ProjectError::Enqueue)
}

/// Returns the build's next state, and whether it did anything.
fn build_step<S, V, Q>(
    snap: &S,
    index: &V,
    vault: &VaultId,
    mut build: Build,
    head: Option<u64>,
    live: &mut Option<Live>,
    enqueue: &mut Q,
) -> Result<(Option<Build>, bool), ProjectError>
where
    S: Snapshot,
    V: VectorIndex,
    Q: FnMut(&[EmbedRequest]) -> Result<(), String>,
{
    match build.stage {
        // Everything after this point is covered by catching up.
        Stage::Start => {
            index.begin_build(vault, &build.model)?;
            build.from = head;
            build.upto = head;
            build.stage = Stage::Scan;
            Ok((Some(build), true))
        }
        Stage::Scan => {
            let page = snap.rows(build.table, build.after.as_ref())?;
            let Some(last) = page.last().map(|r| r.header().id.clone()) else {
                match SCAN_ORDER.iter().position(|t| *t == build.table).and_then(|i| SCAN_ORDER.get(i + 1)) {
                    Some(next) => {
                        build.table = *next;
                        build.after = None;
                    }
                    None => build.stage = Stage::CatchUp,
                }
                return Ok((Some(build), true));
            };
            let ids: Vec<ObjectId> = page.iter().map(|r| r.header().id.clone()).collect();
            let ops = match page.first() {
                Some(Row::Assertion(_)) => assertion_ops(snap, &ids)?,
                Some(Row::Entity(_)) => crate::entity::entity_ops(snap, &ids)?,
                _ => segment_ops(snap, &ids)?,
            };
            let plan = plan_ops(snap, index, vault, &build.model.name, ops)?;
            send(enqueue, &build.model.name, &plan)?;
            index.apply(vault, Generation::Building, &plan.ops, build.from)?;
            build.note(&plan);
            build.after = Some(last);
            Ok((Some(build), true))
        }
        Stage::CatchUp => {
            let mut caught_up = true;
            let mut progress = false;
            if head > build.upto {
                let events = snap.events(build.upto)?;
                caught_up = events.len() < PAGE_LIMIT;
                if let Some(last) = events.last() {
                    let plan = plan_ops(snap, index, vault, &build.model.name, event_ops(snap, &events)?)?;
                    send(enqueue, &build.model.name, &plan)?;
                    index.apply(vault, Generation::Building, &plan.ops, Some(last.seq))?;
                    build.note(&plan);
                    build.upto = Some(last.seq);
                    progress = true;
                }
            }
            // A first build or a rebuild of one model goes live at once, and fills in as jobs finish. A new model waits, since the old one still answers well.
            let replacing_model = live.as_ref().is_some_and(|l| l.model != build.model);
            if !caught_up || (replacing_model && !build.waiting.is_empty()) {
                return Ok((Some(build), progress));
            }
            index.flip(vault)?;
            tracing::info!(vault = %vault, model = %build.model.name, watermark = ?build.upto, waiting = build.waiting.len(), "vector index generation is live");
            *live = Some(Live {
                model: build.model,
                upto: build.upto,
            });
            Ok((None, true))
        }
    }
}
