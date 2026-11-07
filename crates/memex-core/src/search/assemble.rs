//! Fences candidates against the store and turns them into evidence items. Every read is batched, so the store calls don't grow with the number of hits.
use super::budget::item_cost;
use super::fusion::Fused;
use crate::cache::{Caches, NoCaches, is_cached_table, object_key};
use crate::entity::{current_claims, entity_doc, latest_types};
use crate::governance::CONTRADICTS;
use crate::lexical::{assertion_doc, claim_text, current_rendition, filter_passes, segment_doc};
use crate::page::{block_doc, page_doc};
use crate::{Snapshot, StoreError};
use memex_types::{
    AnchorStatus, AssertionRow, BlockRow, Citation, Conflict, DocKind, EntityRow, EntityTypeRow, EvidenceItem, EvidenceRow, FacetCount, Facets, IndexDoc,
    ItemKind, LexicalFilter, ObjectId, PageRow, RenditionRow, Row, RowKey, SegmentRow, SourceRow, Status, Table,
};
use std::collections::{BTreeMap, BTreeSet, HashMap};

/// A candidate that passed the fence, with what ranking and facets need.
#[derive(Debug, Clone, PartialEq)]
pub struct Fenced {
    pub fused: Fused,
    pub item: EvidenceItem,
    pub doc: IndexDoc,
    pub cost: usize,
    pub rerank_score: Option<f32>,
}

/// How many citations one entity carries. There's one per value at most this many, and the rest are a call away on the entity itself.
const ENTITY_CITATIONS: usize = 5;

fn key(table: Table, id: &ObjectId) -> RowKey {
    RowKey { table, id: id.clone() }
}

fn table_of(kind: DocKind) -> Table {
    match kind {
        DocKind::Segment => Table::Segment,
        DocKind::Claim => Table::Assertion,
        DocKind::Entity => Table::Entity,
        DocKind::Page => Table::Page,
        DocKind::Block => Table::Block,
    }
}

fn claim_is_current(a: &AssertionRow) -> bool {
    a.header.status == Status::Active && a.superseded_by.is_none()
}

fn epistemic(a: &AssertionRow) -> Option<String> {
    serde_json::to_value(a.epistemic).ok().and_then(|v| v.as_str().map(String::from))
}

/// Reads rows for the fence, through the objects tier when there is one. A row from the tier is the one this snapshot holds, as its key has the stamp the snapshot gave.
pub struct RowReader<'a, S, Ca = NoCaches> {
    snap: &'a S,
    tier: Option<(&'a Ca, &'a memex_types::VaultId, u64)>,
}

impl<'a, S: Snapshot> RowReader<'a, S, NoCaches> {
    pub fn plain(snap: &'a S) -> Self {
        Self { snap, tier: None }
    }
}

impl<'a, S: Snapshot, Ca: Caches> RowReader<'a, S, Ca> {
    /// `generation` is none when it couldn't be read, and then no tier is used.
    pub fn new(snap: &'a S, caches: &'a Ca, vault: &'a memex_types::VaultId, generation: Option<u64>) -> Self {
        let tier = generation.filter(|_| caches.objects_on()).map(|g| (caches, vault, g));
        Self { snap, tier }
    }

    /// One call for the stamps if any row could be in the tier, and one for what wasn't.
    fn get_many(&self, keys: &[RowKey]) -> Result<Vec<Option<Row>>, StoreError> {
        let Some((caches, vault, generation)) = self.tier else {
            return self.snap.get_many(keys);
        };
        let (held, plain): (Vec<&RowKey>, Vec<&RowKey>) = keys.iter().partition(|k| is_cached_table(k.table));
        if held.is_empty() {
            return self.snap.get_many(keys);
        }
        let held_keys: Vec<RowKey> = held.into_iter().cloned().collect();
        let stamps = self.snap.row_stamps(&held_keys)?;
        let mut found: HashMap<RowKey, Row> = HashMap::with_capacity(keys.len());
        let mut wanted: Vec<RowKey> = plain.into_iter().cloned().collect();
        let mut to_keep: HashMap<RowKey, String> = HashMap::new();
        for (k, stamp) in held_keys.iter().zip(&stamps) {
            // No stamp means no row, which needs no second read.
            let Some(stamp) = stamp else { continue };
            let cache_key = object_key(vault, generation, k.table, &k.id, stamp);
            match caches.get_row(&cache_key) {
                Some(row) => {
                    found.insert(k.clone(), row);
                }
                None => {
                    wanted.push(k.clone());
                    to_keep.insert(k.clone(), cache_key);
                }
            }
        }
        if !wanted.is_empty() {
            for (k, row) in wanted.iter().zip(self.snap.get_many(&wanted)?) {
                if let Some(row) = row {
                    if let Some(cache_key) = to_keep.remove(k) {
                        caches.put_row(cache_key, &row);
                    }
                    found.insert(k.clone(), row);
                }
            }
        }
        Ok(keys.iter().map(|k| found.get(k).cloned()).collect())
    }
}

/// Everything the items need, read in a fixed number of calls.
#[derive(Default)]
struct Loaded {
    segments: HashMap<ObjectId, SegmentRow>,
    assertions: HashMap<ObjectId, AssertionRow>,
    entities: HashMap<ObjectId, EntityRow>,
    entity_types: HashMap<String, EntityTypeRow>,
    sources: HashMap<ObjectId, SourceRow>,
    renditions: HashMap<ObjectId, RenditionRow>,
    evidence: HashMap<ObjectId, Vec<EvidenceRow>>,
    /// Each claim's current rivals.
    conflicts: HashMap<ObjectId, Vec<ObjectId>>,
    pages: HashMap<ObjectId, PageRow>,
    blocks: HashMap<ObjectId, BlockRow>,
}

impl Loaded {
    fn absorb(&mut self, rows: Vec<Option<Row>>) {
        for row in rows.into_iter().flatten() {
            match row {
                Row::Segment(s) => {
                    self.segments.insert(s.header.id.clone(), s);
                }
                Row::Assertion(a) => {
                    self.assertions.insert(a.header.id.clone(), a);
                }
                Row::Entity(e) => {
                    self.entities.insert(e.header.id.clone(), e);
                }
                Row::Source(s) => {
                    self.sources.insert(s.header.id.clone(), s);
                }
                Row::Rendition(r) => {
                    self.renditions.insert(r.header.id.clone(), r);
                }
                Row::Page(p) => {
                    self.pages.insert(p.header.id.clone(), p);
                }
                Row::Block(b) => {
                    self.blocks.insert(b.header.id.clone(), b);
                }
                _ => {}
            }
        }
    }

    fn read<S: Snapshot, Ca: Caches>(rows: &RowReader<'_, S, Ca>, keys: &[RowKey], into: &mut Self) -> Result<(), StoreError> {
        if !keys.is_empty() {
            into.absorb(rows.get_many(keys)?);
        }
        Ok(())
    }

    fn load<S: Snapshot, Ca: Caches>(rows: &RowReader<'_, S, Ca>, candidates: &[Fused]) -> Result<Self, StoreError> {
        let snap = rows.snap;
        let mut l = Self::default();
        let keys: Vec<RowKey> = candidates.iter().map(|c| key(table_of(c.kind), &c.id)).collect();
        Self::read(rows, &keys, &mut l)?;
        l.entities.retain(|_, e| e.is_current());
        // A block's item names its page.
        let keys: Vec<RowKey> = l
            .blocks
            .values()
            .map(|b| &b.page_id)
            .filter(|p| !l.pages.contains_key(*p))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(|p| key(Table::Page, p))
            .collect();
        Self::read(rows, &keys, &mut l)?;
        if !l.entities.is_empty() {
            // An entity's citations are the evidence behind its values, and its conflicts are the claims those values beat.
            let keys: Vec<RowKey> = l
                .entities
                .values()
                .flat_map(|e| e.fields.iter())
                .flat_map(|f| std::iter::once(&f.assertion_id).chain(f.others.iter().map(|o| &o.assertion_id)))
                .collect::<BTreeSet<_>>()
                .into_iter()
                .map(|id| key(Table::Assertion, id))
                .collect();
            Self::read(rows, &keys, &mut l)?;
            let names: Vec<String> = l.entities.values().map(|e| e.type_name.clone()).collect::<BTreeSet<_>>().into_iter().collect();
            l.entity_types = latest_types(snap, &names)?;
        }
        let claims: Vec<ObjectId> = l.assertions.values().filter(|a| claim_is_current(a)).map(|a| a.header.id.clone()).collect();
        if !claims.is_empty() {
            let mut rivals = BTreeSet::new();
            for e in snap.edges_touching(&claims, CONTRADICTS)? {
                for (a, b) in [(&e.from, &e.to), (&e.to, &e.from)] {
                    if claims.contains(a) {
                        l.conflicts.entry(a.clone()).or_default().push(b.clone());
                        if !l.assertions.contains_key(b) {
                            rivals.insert(b.clone());
                        }
                    }
                }
            }
            let keys: Vec<RowKey> = rivals.iter().map(|id| key(Table::Assertion, id)).collect();
            Self::read(rows, &keys, &mut l)?;
            let all: Vec<ObjectId> = l.assertions.keys().cloned().collect();
            let evidence = snap.evidence_of(&all)?;
            let keys: Vec<RowKey> = evidence
                .iter()
                .map(|e| e.segment_id.clone())
                .collect::<BTreeSet<_>>()
                .iter()
                .filter(|id| !l.segments.contains_key(*id))
                .map(|id| key(Table::Segment, id))
                .collect();
            Self::read(rows, &keys, &mut l)?;
            for e in evidence {
                l.evidence.entry(e.assertion_id.clone()).or_default().push(e);
            }
        }
        let mut wanted: BTreeSet<RowKey> = BTreeSet::new();
        for s in l.segments.values() {
            wanted.insert(key(Table::Source, &s.source_id));
            if let Some(r) = &s.rendition_id {
                wanted.insert(key(Table::Rendition, r));
            }
        }
        let keys: Vec<RowKey> = wanted.into_iter().collect();
        Self::read(rows, &keys, &mut l)?;
        Ok(l)
    }

    fn segment_is_current(&self, s: &SegmentRow) -> bool {
        s.header.status == Status::Active && s.rendition_id.as_ref().is_none_or(|id| self.renditions.get(id).is_some_and(current_rendition))
    }

    fn cited<'a>(&'a self, a: &ObjectId) -> Vec<&'a EvidenceRow> {
        self.evidence.get(a).map_or(vec![], |e| e.iter().collect())
    }

    fn source_fields(&self, id: &ObjectId) -> (Option<ObjectId>, Option<String>, Option<String>, Option<String>) {
        match self.sources.get(id) {
            Some(s) => (Some(id.clone()), Some(s.name.clone()), Some(s.sha256.clone()), Some(s.trust_tier.clone())),
            None => (Some(id.clone()), None, None, None),
        }
    }

    fn segment_citation(&self, s: &SegmentRow) -> Citation {
        let (source_id, source_name, source_sha256, trust_tier) = self.source_fields(&s.source_id);
        let located = s.rendition_id.is_some();
        Citation {
            source_id,
            source_name,
            source_sha256,
            trust_tier,
            segment_id: Some(s.header.id.clone()),
            evidence_id: None,
            status: if located { AnchorStatus::Located } else { AnchorStatus::None },
            rendition_id: s.rendition_id.clone(),
            start: located.then_some(s.start),
            end: located.then_some(s.end),
            page_from: s.page_from,
            page_to: s.page_to,
            rects: vec![],
            quote: None,
        }
    }

    fn evidence_citation(&self, e: &EvidenceRow) -> Citation {
        let segment = self.segments.get(&e.segment_id);
        let (source_id, source_name, source_sha256, trust_tier) = segment.map_or((None, None, None, None), |s| self.source_fields(&s.source_id));
        let status = match (&e.anchor, e.anchor_broken) {
            (_, true) => AnchorStatus::Broken,
            (Some(_), false) => AnchorStatus::Located,
            (None, false) => AnchorStatus::None,
        };
        let anchor = e.anchor.as_ref();
        Citation {
            source_id,
            source_name,
            source_sha256,
            trust_tier,
            segment_id: Some(e.segment_id.clone()),
            evidence_id: Some(e.header.id.clone()),
            status,
            rendition_id: anchor.map(|a| a.rendition_id.clone()).or_else(|| segment.and_then(|s| s.rendition_id.clone())),
            start: anchor.map(|a| a.start),
            end: anchor.map(|a| a.end),
            page_from: segment.and_then(|s| s.page_from),
            page_to: segment.and_then(|s| s.page_to),
            rects: anchor.map_or(vec![], |a| a.rects.clone()),
            quote: Some(e.quote.clone()),
        }
    }

    /// A claim nobody cited still says so, in its one citation.
    fn no_anchor() -> Citation {
        Citation {
            source_id: None,
            source_name: None,
            source_sha256: None,
            trust_tier: None,
            segment_id: None,
            evidence_id: None,
            status: AnchorStatus::None,
            rendition_id: None,
            start: None,
            end: None,
            page_from: None,
            page_to: None,
            rects: vec![],
            quote: None,
        }
    }

    fn conflicts_of(&self, claim: &ObjectId) -> Vec<Conflict> {
        let mut out = vec![];
        for rival in self.conflicts.get(claim).into_iter().flatten() {
            let Some(a) = self.assertions.get(rival).filter(|a| claim_is_current(a)) else {
                continue;
            };
            let sources: BTreeSet<ObjectId> = self
                .cited(rival)
                .iter()
                .filter_map(|e| self.segments.get(&e.segment_id))
                .map(|s| s.source_id.clone())
                .collect();
            out.push(Conflict {
                claim_id: rival.clone(),
                text: claim_text(a).trim_end().to_string(),
                epistemic: epistemic(a),
                source_ids: sources.into_iter().collect(),
            });
        }
        out
    }

    fn segment_item(&self, s: &SegmentRow, score: f32, doc: &IndexDoc) -> EvidenceItem {
        finish(EvidenceItem {
            id: s.header.id.clone(),
            kind: ItemKind::Segment,
            text: s.text.clone(),
            heading: s.heading.clone(),
            language: s.language.clone(),
            epistemic: None,
            trust_tiers: doc.trust_tiers.clone(),
            valid_from_ms: None,
            valid_to_ms: None,
            score: Some(score),
            estimated_tokens: 0,
            citations: vec![self.segment_citation(s)],
            conflicts: vec![],
            page_id: None,
        })
    }

    fn entity_doc(&self, e: &EntityRow) -> Option<IndexDoc> {
        let t = self.entity_types.get(&e.type_name)?;
        let cited: Vec<&EvidenceRow> = current_claims(e).flat_map(|c| self.cited(c)).collect();
        Some(entity_doc(e, &t.schema, &cited, &self.segments, &self.sources))
    }

    /// Text and types as the card has them, one citation per current value, up to a limit, and the claims that lost.
    fn entity_item(&self, e: &EntityRow, score: f32, doc: &IndexDoc) -> EvidenceItem {
        let mut citations: Vec<Citation> = current_claims(e)
            .flat_map(|c| self.cited(c))
            .take(ENTITY_CITATIONS)
            .map(|x| self.evidence_citation(x))
            .collect();
        if citations.is_empty() {
            citations.push(Self::no_anchor());
        }
        let mut conflicts = vec![];
        for f in &e.fields {
            for other in f.others.iter().filter(|o| o.value != f.value) {
                let Some(a) = self.assertions.get(&other.assertion_id) else { continue };
                let sources: BTreeSet<ObjectId> = self
                    .cited(&other.assertion_id)
                    .iter()
                    .filter_map(|x| self.segments.get(&x.segment_id))
                    .map(|s| s.source_id.clone())
                    .collect();
                conflicts.push(Conflict {
                    claim_id: other.assertion_id.clone(),
                    text: claim_text(a).trim_end().to_string(),
                    epistemic: epistemic(a),
                    source_ids: sources.into_iter().collect(),
                });
            }
        }
        finish(EvidenceItem {
            id: e.header.id.clone(),
            kind: ItemKind::Entity,
            text: doc.text.clone(),
            heading: Some(e.type_name.clone()),
            language: doc.language.clone(),
            epistemic: None,
            trust_tiers: doc.trust_tiers.clone(),
            valid_from_ms: None,
            valid_to_ms: None,
            score: Some(score),
            estimated_tokens: 0,
            citations,
            conflicts,
            page_id: None,
        })
    }

    fn claim_item(&self, a: &AssertionRow, score: f32, doc: &IndexDoc) -> EvidenceItem {
        let mut citations: Vec<Citation> = self.cited(&a.header.id).iter().map(|e| self.evidence_citation(e)).collect();
        if citations.is_empty() {
            citations.push(Self::no_anchor());
        }
        finish(EvidenceItem {
            id: a.header.id.clone(),
            kind: ItemKind::Claim,
            text: claim_text(a).trim_end().to_string(),
            heading: None,
            language: doc.language.clone(),
            epistemic: epistemic(a),
            trust_tiers: doc.trust_tiers.clone(),
            valid_from_ms: a.valid_from_ms,
            valid_to_ms: a.valid_to_ms,
            score: Some(score),
            estimated_tokens: 0,
            citations,
            conflicts: self.conflicts_of(&a.header.id),
            page_id: None,
        })
    }

    /// A page or a block. Its text is the document's, without link syntax. It has no source, so its one citation says so.
    fn page_item(&self, id: &ObjectId, kind: ItemKind, page: &PageRow, score: f32, doc: &IndexDoc) -> EvidenceItem {
        finish(EvidenceItem {
            id: id.clone(),
            kind,
            text: doc.text.clone(),
            heading: Some(page.title.clone()),
            language: doc.language.clone(),
            epistemic: None,
            trust_tiers: vec![],
            valid_from_ms: None,
            valid_to_ms: None,
            score: Some(score),
            estimated_tokens: 0,
            citations: vec![Self::no_anchor()],
            conflicts: vec![],
            page_id: Some(page.header.id.clone()),
        })
    }
}

fn finish(mut item: EvidenceItem) -> EvidenceItem {
    item.estimated_tokens = item_cost(&item);
    item
}

/// Keeps what's still current and still passes the filter, as of this snapshot, and builds its item. The index may be behind, and a row may have changed since it was indexed: this is the one place that decides what's returned.
pub fn fence<S: Snapshot>(snap: &S, candidates: Vec<Fused>, filter: &LexicalFilter) -> Result<Vec<Fenced>, StoreError> {
    fence_with(&RowReader::plain(snap), candidates, filter)
}

/// The fence, with its rows read through `rows`.
pub fn fence_with<S: Snapshot, Ca: Caches>(rows: &RowReader<'_, S, Ca>, candidates: Vec<Fused>, filter: &LexicalFilter) -> Result<Vec<Fenced>, StoreError> {
    if candidates.is_empty() {
        return Ok(vec![]);
    }
    let loaded = Loaded::load(rows, &candidates)?;
    let mut out = Vec::with_capacity(candidates.len());
    for fused in candidates {
        let built = match fused.kind {
            DocKind::Segment => loaded.segments.get(&fused.id).filter(|s| loaded.segment_is_current(s)).map(|s| {
                let doc = segment_doc(s.clone(), loaded.sources.get(&s.source_id));
                (loaded.segment_item(s, fused.score, &doc), doc)
            }),
            DocKind::Claim => loaded.assertions.get(&fused.id).filter(|a| claim_is_current(a)).map(|a| {
                let cited = loaded.cited(&a.header.id);
                let doc = assertion_doc(a, &cited, &loaded.segments, &loaded.sources);
                (loaded.claim_item(a, fused.score, &doc), doc)
            }),
            DocKind::Entity => loaded.entities.get(&fused.id).and_then(|e| {
                let doc = loaded.entity_doc(e)?;
                Some((loaded.entity_item(e, fused.score, &doc), doc))
            }),
            // The fence reads no headings for a page's item: its document's search text was only for finding it.
            DocKind::Page => loaded.pages.get(&fused.id).map(|p| {
                let doc = page_doc(p, &[]);
                (loaded.page_item(&fused.id, ItemKind::Page, p, fused.score, &doc), doc)
            }),
            DocKind::Block => loaded.blocks.get(&fused.id).and_then(|b| {
                let doc = block_doc(b)?;
                let page = loaded.pages.get(&b.page_id)?;
                Some((loaded.page_item(&fused.id, ItemKind::Block, page, fused.score, &doc), doc))
            }),
        };
        if let Some((item, doc)) = built
            && filter_passes(&doc, filter)
        {
            let cost = item.estimated_tokens;
            out.push(Fenced {
                fused,
                item,
                doc,
                cost,
                rerank_score: None,
            });
        }
    }
    Ok(out)
}

fn count(values: impl Iterator<Item = String>) -> Vec<FacetCount> {
    let mut counts: BTreeMap<String, u64> = BTreeMap::new();
    for v in values {
        *counts.entry(v).or_default() += 1;
    }
    let mut out: Vec<FacetCount> = counts.into_iter().map(|(value, count)| FacetCount { value, count }).collect();
    out.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.value.cmp(&b.value)));
    out
}

pub fn facets(candidates: &[Fenced]) -> Facets {
    Facets {
        kind: count(candidates.iter().map(|c| c.doc.kind.as_str().to_string())),
        doc_type: count(candidates.iter().filter_map(|c| c.doc.doc_type.clone())),
        source_id: count(candidates.iter().flat_map(|c| c.doc.source_ids.iter().map(|s| s.0.clone()))),
        trust_tier: count(candidates.iter().flat_map(|c| c.doc.trust_tiers.iter().cloned())),
        epistemic: count(candidates.iter().filter_map(|c| c.doc.epistemic.clone())),
    }
}

/// How many of a page's segments get their section as context.
const SECTIONS_PER_PAGE: usize = 3;
/// A section is context, so a very long one isn't read in full.
const SECTION_ROWS: usize = 200;

/// The parent sections of the page's best segments, as far as the remaining budget allows. One store call.
pub fn parent_sections<S: Snapshot>(snap: &S, page: &[&Fenced], budget_left: usize) -> Result<Vec<EvidenceItem>, StoreError> {
    let wanted: Vec<(ObjectId, String)> = page
        .iter()
        .filter(|c| c.item.kind == ItemKind::Segment)
        .filter_map(|c| {
            let citation = c.item.citations.first()?;
            Some((citation.rendition_id.clone()?, c.item.heading.clone().filter(|h| !h.is_empty())?))
        })
        .collect::<Vec<_>>()
        .into_iter()
        .fold(vec![], |mut seen: Vec<(ObjectId, String)>, s| {
            if !seen.contains(&s) {
                seen.push(s);
            }
            seen
        })
        .into_iter()
        .take(SECTIONS_PER_PAGE)
        .collect();
    if wanted.is_empty() {
        return Ok(vec![]);
    }
    let rows = snap.sections_of(&wanted, SECTION_ROWS)?;
    let mut left = budget_left;
    let mut out = vec![];
    for (rendition, heading) in &wanted {
        let parts: Vec<&SegmentRow> = rows
            .iter()
            .filter(|s| s.rendition_id.as_ref() == Some(rendition) && s.heading.as_ref() == Some(heading) && s.header.status == Status::Active)
            .collect();
        // A section of one segment says nothing the hit didn't.
        let (Some(first), Some(last), true) = (parts.first(), parts.last(), parts.len() > 1) else {
            continue;
        };
        let text = parts.iter().map(|s| s.text.as_str()).collect::<Vec<_>>().join("\n\n");
        let page_from = parts.iter().filter_map(|s| s.page_from).min();
        let page_to = parts.iter().filter_map(|s| s.page_to).max();
        let hit = page
            .iter()
            .find(|c| c.item.citations.first().and_then(|x| x.rendition_id.as_ref()) == Some(rendition) && c.item.heading.as_ref() == Some(heading));
        let mut citation = hit.and_then(|c| c.item.citations.first().cloned()).unwrap_or_else(Loaded::no_anchor);
        citation.segment_id = None;
        citation.status = AnchorStatus::Located;
        citation.rendition_id = Some(rendition.clone());
        citation.start = Some(first.start);
        citation.end = Some(last.end);
        citation.page_from = page_from;
        citation.page_to = page_to;
        let item = finish(EvidenceItem {
            id: first.header.id.clone(),
            kind: ItemKind::Section,
            text,
            heading: Some(heading.clone()),
            language: first.language.clone(),
            epistemic: None,
            trust_tiers: citation.trust_tier.clone().into_iter().collect(),
            valid_from_ms: None,
            valid_to_ms: None,
            score: None,
            estimated_tokens: 0,
            citations: vec![citation],
            conflicts: vec![],
            page_id: None,
        });
        if item.estimated_tokens <= left {
            left -= item.estimated_tokens;
            out.push(item);
        }
    }
    Ok(out)
}

/// Text for the cross-encoder: what a claim states, with its quotes, or a segment's text.
pub fn rerank_text(c: &Fenced) -> &str {
    &c.doc.text
}
