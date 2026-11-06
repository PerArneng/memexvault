//! One search, from request to evidence package.
use super::assemble::{Fenced, facets, fence, parent_sections, rerank_text};
use super::budget::{decode_cursor, encode_cursor, fit};
use super::fusion::{Fused, RRF_K, fuse};
use super::history::{HistoryError, keyword_scan, past_docs, past_state};
use super::plan::{CANDIDATES_PER_SIGNAL, QueryPlan, plan_query};
use super::relevance::{Signals, THRESHOLDS, is_low_relevance, term_coverage};
use super::{RERANK_TOP, Reranker};
use crate::lexical::MAX_SEARCH_LIMIT;
use crate::{Clock, Embedder, LexicalIndex, ServerError, Snapshot, Store, Touched, VectorIndex};
use memex_types::{
    BudgetReport, DocKind, EmbedPurpose, EvidenceItem, EvidencePackage, Generation, ItemKind, LexicalError, LexicalFilter, LexicalQuery, LexicalResults,
    ObjectId, QueryPath, SearchDebug, SearchMode, SearchRequest, SignalScores, VaultId, VectorError, VectorHealth, VectorQuery,
};
use std::time::Duration;

/// How many of the best items a word coverage check reads.
const COVERAGE_ITEMS: usize = 5;
/// Candidates fenced beyond what a page needs, since a few may have gone. Reading every row a signal offered took most of the fence's time, and few pages get past the first ranks.
const FENCE_SLACK: usize = 10;

/// How many of the best candidates this page has to fence.
fn fence_window(req: &SearchRequest, offset: usize) -> usize {
    if req.facets {
        // Facets count every candidate.
        return usize::MAX;
    }
    let needed = offset.saturating_add(req.limit);
    needed.max(if req.rerank { RERANK_TOP } else { 0 }).saturating_add(FENCE_SLACK)
}

/// Fences the best candidates. If too many of them turn out gone to fill the page, it fences the rest as well, which costs one more set of reads. Returns the items, and whether there are candidates it didn't read.
fn fence_page<S: Snapshot>(snap: &S, mut fused: Vec<Fused>, req: &SearchRequest, offset: usize) -> Result<(Vec<Fenced>, bool), ServerError> {
    let window = fence_window(req, offset);
    if fused.len() <= window {
        return Ok((fence(snap, fused, &req.filter)?, false));
    }
    let rest = fused.split_off(window);
    let mut fenced = fence(snap, fused, &req.filter)?;
    if fenced.len() >= offset.saturating_add(req.limit) {
        return Ok((fenced, true));
    }
    fenced.extend(fence(snap, rest, &req.filter)?);
    Ok((fenced, false))
}

pub struct SearchEdgesRef<'a, St, L, V, Em, R, C> {
    pub store: &'a St,
    pub lexical: &'a L,
    pub vector: &'a V,
    pub embedder: &'a Em,
    pub reranker: &'a R,
    pub clock: &'a C,
}

/// Records how long each stage took, only when asked, so an ordinary search doesn't pay for it.
struct Stages<'c, C> {
    clock: Option<&'c C>,
    last: Duration,
    out: Vec<(String, f64)>,
}

impl<'c, C: Clock> Stages<'c, C> {
    fn new(clock: &'c C, on: bool) -> Self {
        Self {
            clock: on.then_some(clock),
            last: if on { clock.monotonic() } else { Duration::ZERO },
            out: vec![],
        }
    }

    fn mark(&mut self, stage: &str) {
        if let Some(clock) = self.clock {
            let now = clock.monotonic();
            self.out.push((stage.to_string(), now.saturating_sub(self.last).as_secs_f64() * 1000.0));
            self.last = now;
        }
    }
}

/// The vector signal: embeds the query with the live generation's model, then searches. A model flip between the two costs one more try.
pub(crate) fn vector_candidates<V: VectorIndex, Em: Embedder>(
    vector: &V,
    embedder: &Em,
    vault: &VaultId,
    text: &str,
    filter: &LexicalFilter,
    limit: usize,
) -> Result<LexicalResults, ServerError> {
    let mut last = None;
    for _ in 0..2 {
        let model = match vector.health(vault)? {
            VectorHealth::Ready { model, .. } => model,
            VectorHealth::Missing => return Err(ServerError::Unavailable("the vector index is still being built".into())),
        };
        let embedded = embedder
            .embed(&model.name, EmbedPurpose::Query, &[text.to_string()])?
            .pop()
            .ok_or_else(|| ServerError::Unavailable("the embedder returned nothing".into()))?;
        let query = VectorQuery {
            model: model.name,
            vector: embedded,
            filter: filter.clone(),
            limit: limit.min(MAX_SEARCH_LIMIT),
        };
        match vector.search(vault, &query) {
            Ok(results) => return Ok(results),
            Err(e @ VectorError::ModelMismatch { .. }) => last = Some(e),
            Err(e) => return Err(e.into()),
        }
    }
    Err(last.map_or_else(|| ServerError::Unavailable("the vector index changed model".into()), Into::into))
}

fn lexical_candidates<L: LexicalIndex>(lexical: &L, vault: &VaultId, plan: &QueryPlan, filter: &LexicalFilter) -> Result<LexicalResults, ServerError> {
    let query = LexicalQuery {
        terms: plan.terms.clone(),
        filter: filter.clone(),
        limit: CANDIDATES_PER_SIGNAL,
    };
    Ok(lexical.search(vault, &query)?)
}

/// A signal's failure degrades a hybrid search, as the other can still answer. A bad request fails it, since the other signal would be just as wrong to run.
fn degradable(e: &ServerError) -> bool {
    !matches!(e, ServerError::Invalid(_))
}

struct Gathered {
    lexical: Vec<memex_types::LexicalHit>,
    vector: Vec<memex_types::LexicalHit>,
    watermark: Option<u64>,
    degraded: Vec<String>,
}

fn gather<L, V, Em>(
    lexical: &L,
    vector: &V,
    embedder: &Em,
    vault: &VaultId,
    text: &str,
    plan: &QueryPlan,
    filter: &LexicalFilter,
) -> Result<Gathered, ServerError>
where
    L: LexicalIndex + Sync,
    V: VectorIndex + Sync,
    Em: Embedder + Sync,
{
    let run_lexical = || lexical_candidates(lexical, vault, plan, filter);
    let run_vector = || vector_candidates(vector, embedder, vault, text, filter, CANDIDATES_PER_SIGNAL);
    let (from_lexical, from_vector) = match (plan.use_lexical, plan.use_vector) {
        (true, true) => std::thread::scope(|scope| {
            let other = scope.spawn(run_vector);
            let here = run_lexical();
            (
                Some(here),
                Some(
                    other
                        .join()
                        .unwrap_or_else(|_| Err(ServerError::Unavailable("the vector search stopped".into()))),
                ),
            )
        }),
        (true, false) => (Some(run_lexical()), None),
        (false, _) => (None, Some(run_vector())),
    };
    let both = plan.use_lexical && plan.use_vector;
    let mut out = Gathered {
        lexical: vec![],
        vector: vec![],
        watermark: None,
        degraded: vec![],
    };
    let mut marks: Vec<Option<u64>> = vec![];
    let mut failure = None;
    for (name, result) in [("lexical", from_lexical), ("vector", from_vector)] {
        match result {
            None => {}
            Some(Ok(r)) => {
                marks.push(r.watermark);
                if name == "lexical" {
                    out.lexical = r.hits;
                } else {
                    out.vector = r.hits;
                }
            }
            Some(Err(e)) if both && degradable(&e) => {
                tracing::warn!(signal = name, error = %e, "a search signal failed, so the result rests on the other");
                out.degraded.push(name.to_string());
                failure = Some(e);
            }
            Some(Err(e)) => return Err(e),
        }
    }
    if out.degraded.len() == 2
        && let Some(e) = failure
    {
        return Err(e);
    }
    out.watermark = marks.into_iter().min().flatten();
    Ok(out)
}

/// Whether this index has seen `min_seq`, directly or because nothing it cares about happened since its watermark. An index waits to commit when only unrelated events arrive, and that shouldn't hold a reader up.
fn caught_up<S: Snapshot>(snap: &S, watermark: Option<u64>, min_seq: u64) -> Result<bool, ServerError> {
    if watermark.is_some_and(|w| w >= min_seq) {
        return Ok(true);
    }
    let events = snap.events(watermark)?;
    let mut touched = Touched::default();
    let mut upto = watermark;
    for e in events.iter().take_while(|e| e.seq <= min_seq) {
        touched.add_event(e).map_err(|err| ServerError::Unavailable(err.to_string()))?;
        upto = Some(e.seq);
    }
    Ok(upto.is_some_and(|u| u >= min_seq) && touched.is_empty())
}

/// Errs unless the indexes this search uses have seen `min_seq`. It never waits: the frontend retries for as long as the caller allows.
fn check_min_seq<St, L, V>(store: &St, lexical: &L, vector: &V, vault: &VaultId, min_seq: u64, plan: &QueryPlan) -> Result<(), ServerError>
where
    St: Store,
    L: LexicalIndex,
    V: VectorIndex,
{
    let snap = store.snapshot(vault)?;
    let head = snap.head()?;
    if head.is_none_or(|h| min_seq > h) {
        return Err(ServerError::Invalid(format!(
            "min_seq {min_seq} is ahead of the log, whose last event is {head:?}"
        )));
    }
    let lexical_mark = if plan.use_lexical {
        match lexical.watermark(vault, Generation::Live) {
            Ok(w) => Some(w),
            Err(LexicalError::Unavailable(_) | LexicalError::Invalid(_)) => Some(None),
            Err(e) => return Err(e.into()),
        }
    } else {
        None
    };
    let vector_mark = if plan.use_vector {
        match vector.health(vault)? {
            VectorHealth::Ready { watermark, .. } => Some(watermark),
            VectorHealth::Missing => Some(None),
        }
    } else {
        None
    };
    for mark in [lexical_mark, vector_mark].into_iter().flatten() {
        if !caught_up(&snap, mark, min_seq)? {
            return Err(ServerError::NotCaughtUp { min_seq, reached: mark });
        }
    }
    Ok(())
}

fn signals_of(plan: &QueryPlan, candidates: &[Fenced]) -> Signals {
    let best_vector = plan
        .use_vector
        .then(|| candidates.iter().filter_map(|c| c.fused.vector_score).reduce(f32::max))
        .flatten();
    let coverage = if plan.use_lexical {
        let texts: Vec<&str> = candidates.iter().take(COVERAGE_ITEMS).map(|c| c.doc.text.as_str()).collect();
        term_coverage(&plan.terms, &texts)
    } else {
        None
    };
    Signals { best_vector, coverage }
}

fn rerank<R: Reranker>(reranker: &R, text: &str, candidates: &mut [Fenced], degraded: &mut Vec<String>) {
    let top = candidates.len().min(RERANK_TOP);
    if top == 0 {
        return;
    }
    let passages: Vec<&str> = candidates[..top].iter().map(rerank_text).collect();
    match reranker.rerank(text, &passages) {
        Ok(scores) if scores.len() == top => {
            for (c, score) in candidates.iter_mut().zip(scores) {
                c.rerank_score = Some(score);
                c.item.score = Some(score);
            }
            candidates[..top].sort_by(|a, b| {
                b.rerank_score
                    .unwrap_or(f32::MIN)
                    .total_cmp(&a.rerank_score.unwrap_or(f32::MIN))
                    .then_with(|| a.fused.id.cmp(&b.fused.id))
            });
        }
        Ok(scores) => {
            tracing::warn!(asked = top, got = scores.len(), "the reranker returned the wrong number of scores");
            degraded.push("rerank".into());
        }
        Err(e) => {
            tracing::warn!(error = %e, "reranking failed, so the fused order stands");
            degraded.push("rerank".into());
        }
    }
}

fn debug_of(plan: &QueryPlan, signals: Signals, before: usize, candidates: &[Fenced], stages: Vec<(String, f64)>) -> SearchDebug {
    SearchDebug {
        lexical_weight: plan.lexical_weight,
        vector_weight: plan.vector_weight,
        lexical_terms: plan.terms.len(),
        dropped_terms: plan.dropped_terms,
        best_vector_score: signals.best_vector,
        term_coverage: signals.coverage,
        candidates_before_fence: before,
        candidates_after_fence: candidates.len(),
        signals: candidates
            .iter()
            .map(|c| SignalScores {
                id: c.fused.id.clone(),
                lexical_rank: c.fused.lexical_rank,
                lexical_score: c.fused.lexical_score,
                vector_rank: c.fused.vector_rank,
                vector_score: c.fused.vector_score,
                fused: c.fused.score,
                rerank_score: c.rerank_score,
            })
            .collect(),
        timings_ms: stages,
    }
}

/// An id could be either kind, and the fence drops whichever it isn't.
fn exact_candidates(id: &ObjectId) -> Vec<Fused> {
    [DocKind::Segment, DocKind::Claim]
        .map(|kind| Fused {
            id: id.clone(),
            kind,
            score: 1.0,
            lexical_rank: None,
            lexical_score: None,
            vector_rank: None,
            vector_score: None,
        })
        .to_vec()
}

/// Everything after the candidates: the page, its context and the report on it.
struct Finish<'r, 'c, C> {
    req: &'r SearchRequest,
    plan: &'r QueryPlan,
    path: QueryPath,
    offset: usize,
    before_fence: usize,
    watermark: Option<u64>,
    as_of: Option<u64>,
    degraded: Vec<String>,
    notices: Vec<String>,
    low_relevance: bool,
    signals: Signals,
    stages: Stages<'c, C>,
    /// Good candidates exist past the ones fenced.
    more_unread: bool,
    /// The sections around the hits, which someone asking for one object by id doesn't need.
    context: bool,
}

impl<C: Clock> Finish<'_, '_, C> {
    fn package<S: Snapshot>(mut self, snap: &S, candidates: Vec<Fenced>) -> Result<EvidencePackage, ServerError> {
        let costs: Vec<usize> = candidates.iter().map(|c| c.cost).collect();
        let page = fit(&costs, self.offset, self.req.limit, self.req.budget_tokens);
        let on_page: Vec<&Fenced> = candidates[page.start..page.end].iter().collect();
        let mut items: Vec<EvidenceItem> = on_page.iter().filter(|c| c.item.kind == ItemKind::Claim).map(|c| c.item.clone()).collect();
        items.extend(on_page.iter().filter(|c| c.item.kind != ItemKind::Claim).map(|c| c.item.clone()));
        let mut report: BudgetReport = page.report;
        // Context comes only when the page isn't short of room.
        if self.context && !report.truncated && !on_page.is_empty() {
            let sections = parent_sections(snap, &on_page, report.limit.saturating_sub(report.used))?;
            report.used += sections.iter().map(|s| s.estimated_tokens).sum::<usize>();
            items.extend(sections);
        }
        self.stages.mark("assemble");
        if let Some(needs) = report.smallest_item_needs {
            self.notices.push(format!(
                "the budget of {} tokens is too small for the next item, which needs about {needs}",
                report.limit
            ));
        }
        if candidates.is_empty() {
            self.notices.push("nothing relevant was found".into());
        } else if self.low_relevance {
            self.notices.push("these results don't look like an answer to the question".into());
        }
        let next_cursor = (page.end < candidates.len() || self.more_unread).then(|| encode_cursor(page.end));
        let facet_counts = self.req.facets.then(|| facets(&candidates));
        let debug = self
            .req
            .debug
            .then(|| debug_of(self.plan, self.signals, self.before_fence, &candidates, std::mem::take(&mut self.stages.out)));
        Ok(EvidencePackage {
            path: self.path,
            items,
            low_relevance: self.low_relevance || candidates.is_empty(),
            notices: self.notices,
            budget: report,
            next_cursor,
            watermark: self.watermark,
            as_of: self.as_of,
            degraded: self.degraded,
            debug,
            facets: facet_counts,
        })
    }
}

fn search_past<St: Store, C: Clock>(
    store: &St,
    clock: &C,
    vault: &VaultId,
    req: &SearchRequest,
    plan: &QueryPlan,
    offset: usize,
) -> Result<EvidencePackage, ServerError> {
    let as_of = req.as_of.ok_or_else(|| ServerError::Invalid("as_of is missing".into()))?;
    let mut stages = Stages::new(clock, req.debug);
    let snap = store.snapshot(vault)?;
    let past = past_state(&snap, vault, as_of).map_err(|e| match e {
        HistoryError::AheadOfLog { asked, head } => ServerError::Invalid(format!("as_of {asked} is ahead of the log, whose last event is {head:?}")),
        HistoryError::Store(e) => e.into(),
        HistoryError::Replay(e) => ServerError::Unavailable(e.to_string()),
    })?;
    drop(snap);
    stages.mark("replay");
    let then = past.store.snapshot(vault)?;
    let docs = past_docs(&then, &req.filter)?;
    let hits = keyword_scan(&docs, &plan.terms, CANDIDATES_PER_SIGNAL);
    stages.mark("scan");
    let before_fence = hits.len();
    let (candidates, more_unread) = fence_page(&then, fuse(&hits, &[], 1.0, 1.0, RRF_K), req, offset)?;
    stages.mark("fence");
    let mut notices = vec![];
    if past.seq.is_none() {
        notices.push("that's before the vault's first event, so it held nothing".to_string());
    }
    let finish = Finish {
        req,
        plan,
        path: QueryPath::AsOf,
        offset,
        before_fence,
        watermark: past.seq,
        as_of: past.seq,
        degraded: vec![],
        notices,
        low_relevance: candidates.is_empty(),
        signals: Signals::default(),
        stages,
        more_unread,
        context: true,
    };
    let package = finish.package(&then, candidates);
    drop(then);
    let _ = past.store.close_vault(&past.lease);
    package
}

/// Runs one search. The caller has already checked access.
pub fn run_search<St, L, V, Em, R, C>(e: &SearchEdgesRef<'_, St, L, V, Em, R, C>, vault: &VaultId, req: &SearchRequest) -> Result<EvidencePackage, ServerError>
where
    St: Store,
    L: LexicalIndex + Sync,
    V: VectorIndex + Sync,
    Em: Embedder + Sync,
    R: Reranker,
    C: Clock,
{
    let plan = plan_query(&req.text, &req.filter, req.mode).map_err(|e| ServerError::Invalid(e.to_string()))?;
    let offset = match &req.cursor {
        Some(c) => decode_cursor(c).ok_or_else(|| ServerError::Invalid("that cursor isn't one this API gave out".into()))?,
        None => 0,
    };
    if req.as_of.is_some() {
        if req.mode == SearchMode::Vector {
            return Err(ServerError::Invalid(
                "as_of reads the log, which has no vectors, so it can't use the vector mode".into(),
            ));
        }
        if req.rerank || req.min_seq.is_some() {
            return Err(ServerError::Invalid("as_of can't be combined with rerank or min_seq".into()));
        }
        return search_past(e.store, e.clock, vault, req, &plan, offset);
    }
    let mut stages = Stages::new(e.clock, req.debug);
    if let Some(min_seq) = req.min_seq {
        check_min_seq(e.store, e.lexical, e.vector, vault, min_seq, &plan)?;
    }
    // An id goes straight to the store, and an id nobody has falls through to an ordinary search.
    if let Some(id) = &plan.exact_id {
        let snap = e.store.snapshot(vault)?;
        let candidates = fence(&snap, exact_candidates(id), &req.filter)?;
        if !candidates.is_empty() {
            stages.mark("exact");
            let finish = Finish {
                req,
                plan: &plan,
                path: QueryPath::Exact,
                offset,
                before_fence: 1,
                watermark: None,
                as_of: None,
                degraded: vec![],
                notices: vec![],
                low_relevance: false,
                signals: Signals::default(),
                stages,
                more_unread: false,
                context: false,
            };
            return finish.package(&snap, candidates);
        }
    }
    let gathered = gather(e.lexical, e.vector, e.embedder, vault, &req.text, &plan, &req.filter)?;
    stages.mark("candidates");
    let fused = fuse(&gathered.lexical, &gathered.vector, plan.lexical_weight, plan.vector_weight, RRF_K);
    let before_fence = fused.len().min(fence_window(req, offset));
    // The snapshot is taken after the candidates on purpose: anything retracted while they were gathered is gone from it.
    let snap = e.store.snapshot(vault)?;
    let (mut candidates, more_unread) = fence_page(&snap, fused, req, offset)?;
    stages.mark("fence");
    // Read before any reranking, so asking for it never changes the verdict.
    let signals = signals_of(&plan, &candidates);
    let mut degraded = gathered.degraded;
    if req.rerank {
        rerank(e.reranker, &req.text, &mut candidates, &mut degraded);
        stages.mark("rerank");
    }
    let low_relevance = is_low_relevance(signals, THRESHOLDS);
    let finish = Finish {
        req,
        plan: &plan,
        path: if plan.exact_id.is_some() { QueryPath::Natural } else { plan.path },
        offset,
        before_fence,
        watermark: gathered.watermark,
        as_of: None,
        degraded,
        notices: vec![],
        low_relevance,
        signals,
        stages,
        more_unread,
        context: true,
    };
    finish.package(&snap, candidates)
}
