//! Erasure (spec 030): what removing a document takes with it, worked out from the store, and how the log is rewritten to match.
//!
//! A plan lists ids only. Rows that came from the document go, and rows that hold something of it while standing on other things too are
//! rewritten by pure functions of the removed set. The same functions rewrite every event in the log, and the `source.erased` event's
//! reducer applies them to the live rows, so replaying the rewritten log gives exactly the state the erasure left.
use crate::{PAGE_LIMIT, Snapshot, StoreError};
use memex_types::{
    ChangeSetRow, DerivationRow, EdgeRow, EntityRow, ErasureRecord, Event, EvidenceInput, JobRow, Mutation, ObjectId, Op, Row, RowKey, SOURCE_ERASED, Table,
    entity_of_subject, entity_subject,
};
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashSet};

/// What a plan removes, for the rewrite functions to ask about.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Removed {
    keys: HashSet<RowKey>,
    ids: HashSet<ObjectId>,
}

impl Removed {
    pub fn new(keys: impl IntoIterator<Item = RowKey>) -> Self {
        let keys: HashSet<RowKey> = keys.into_iter().collect();
        let ids = keys.iter().map(|k| k.id.clone()).collect();
        Self { keys, ids }
    }

    pub fn of(record: &ErasureRecord) -> Self {
        Self::new(record.removed.iter().cloned())
    }

    pub fn has(&self, table: Table, id: &ObjectId) -> bool {
        self.ids.contains(id) && self.keys.contains(&RowKey { table, id: id.clone() })
    }

    pub fn has_key(&self, key: &RowKey) -> bool {
        self.keys.contains(key)
    }

    /// Whether any id it removes appears in this JSON, as a whole string or as a `:` or `/` separated part of one, like `entity:<id>`.
    pub fn mentioned_in(&self, value: &Value) -> bool {
        match value {
            Value::String(s) => self.ids.contains(&ObjectId(s.clone())) || s.split([':', '/', ' ']).any(|part| self.ids.contains(&ObjectId(part.into()))),
            Value::Array(items) => items.iter().any(|v| self.mentioned_in(v)),
            Value::Object(map) => map.values().any(|v| self.mentioned_in(v)),
            _ => false,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.keys.is_empty()
    }
}

/// An erasure worked out, with what its physical cascade needs beside the record.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ErasurePlan {
    pub record: ErasureRecord,
    /// Blobs only the removed rows refer to.
    pub blobs: Vec<String>,
    /// Content hashes of texts that go or change, whose stored embeddings go.
    pub embeddings: Vec<String>,
}

fn key(table: Table, id: &ObjectId) -> RowKey {
    RowKey { table, id: id.clone() }
}

fn all_rows<S: Snapshot>(snap: &S, table: Table, mut each: impl FnMut(Row)) -> Result<(), StoreError> {
    let mut after: Option<ObjectId> = None;
    loop {
        let page = snap.rows(table, after.as_ref())?;
        let Some(last) = page.last() else { return Ok(()) };
        after = Some(last.header().id.clone());
        for r in page {
            each(r);
        }
    }
}

fn all_jobs<S: Snapshot>(snap: &S, mut each: impl FnMut(JobRow)) -> Result<(), StoreError> {
    let mut after: Option<ObjectId> = None;
    loop {
        let page = snap.jobs(None, None, after.as_ref(), PAGE_LIMIT)?;
        let Some(last) = page.last() else { return Ok(()) };
        after = Some(last.header.id.clone());
        for j in page {
            each(j);
        }
    }
}

/// Whether an edge is a page's own link, which stays with its page however the thing it points at changes.
fn page_edge(e: &EdgeRow) -> bool {
    e.block_id.is_some() || e.kind == memex_types::WIKI_LINK || e.kind == memex_types::CLAIM_REF
}

/// Works out an erasure of this document, every version of it, from one snapshot. Reads in pages, and lists everything in key order.
pub fn plan<S: Snapshot>(snap: &S, source: &ObjectId) -> Result<Option<ErasurePlan>, StoreError> {
    let Some(Row::Source(named)) = snap.get(&key(Table::Source, source))? else {
        return Ok(None);
    };
    let mut removed: BTreeSet<RowKey> = BTreeSet::new();
    let mut versions = snap.source_versions(&named.logical_id)?;
    if !versions.iter().any(|v| v.header.id == named.header.id) {
        versions.push(named.clone());
    }
    let sources: Vec<ObjectId> = versions.iter().map(|v| v.header.id.clone()).collect();
    let mut blob_candidates: BTreeSet<String> = versions.iter().map(|v| v.sha256.clone()).collect();
    let mut renditions = vec![];
    for s in &sources {
        removed.insert(key(Table::Source, s));
        renditions.extend(snap.renditions_of(s)?);
    }
    let mut segments: Vec<ObjectId> = vec![];
    for r in &renditions {
        removed.insert(key(Table::Rendition, &r.header.id));
        removed.insert(key(Table::Extraction, &r.header.id));
        blob_candidates.insert(r.text_blob.clone());
        blob_candidates.insert(r.words_blob.clone());
        let mut after = None;
        loop {
            let page = snap.segments_of(&r.header.id, after.as_ref(), PAGE_LIMIT)?;
            let Some(last) = page.last() else { break };
            after = Some(last.header.id.clone());
            segments.extend(page.into_iter().map(|s| s.header.id));
        }
        let mut after = None;
        loop {
            let page = snap.derivations_of(&r.header.id, after.as_ref(), PAGE_LIMIT)?;
            let Some(last) = page.last() else { break };
            after = Some(last.header.id.clone());
            removed.extend(page.iter().map(|d| key(Table::Derivation, &d.header.id)));
        }
    }
    for s in &sources {
        segments.extend(snap.segments_of_source(s)?);
    }
    segments.sort();
    segments.dedup();
    removed.extend(segments.iter().map(|s| key(Table::Segment, s)));
    // A removed key that names no row is harmless, but the record should list what really goes.
    let extraction_keys: Vec<RowKey> = removed.iter().filter(|k| k.table == Table::Extraction).cloned().collect();
    for (k, row) in extraction_keys.iter().zip(snap.get_many(&extraction_keys)?) {
        if row.is_none() {
            removed.remove(k);
        }
    }

    // Evidence citing the document, and the claims that stood on nothing else.
    let mut evidence = vec![];
    for chunk in segments.chunks(PAGE_LIMIT) {
        evidence.extend(snap.evidence_citing(chunk)?);
    }
    let gone_evidence: HashSet<ObjectId> = evidence.iter().map(|e| e.header.id.clone()).collect();
    removed.extend(gone_evidence.iter().map(|e| key(Table::Evidence, e)));
    let claims: Vec<ObjectId> = evidence.iter().map(|e| e.assertion_id.clone()).collect::<BTreeSet<_>>().into_iter().collect();
    let mut flagged_claims = vec![];
    let mut gone_claims: Vec<ObjectId> = vec![];
    for chunk in claims.chunks(PAGE_LIMIT) {
        let all = snap.evidence_of(chunk)?;
        for c in chunk {
            if all.iter().filter(|e| &e.assertion_id == c).all(|e| gone_evidence.contains(&e.header.id)) {
                gone_claims.push(c.clone());
            } else {
                flagged_claims.push(c.clone());
            }
        }
    }
    let existing: Vec<RowKey> = gone_claims.iter().map(|c| key(Table::Assertion, c)).collect();
    let mut gone_rows = vec![];
    for chunk in existing.chunks(PAGE_LIMIT) {
        gone_rows.extend(snap.get_many(chunk)?.into_iter().flatten());
    }
    removed.extend(existing);

    // Entities whose every standing claim goes with it go too, since what's left of them came with those claims. Others are worked out again.
    let mut touched_entities: BTreeSet<ObjectId> = BTreeSet::new();
    for r in &gone_rows {
        if let Row::Assertion(a) = r
            && let Some(e) = entity_of_subject(&a.subject)
        {
            touched_entities.insert(e);
        }
    }
    let gone_claim_set: HashSet<ObjectId> = gone_claims.iter().cloned().collect();
    let mut rewritten: BTreeSet<RowKey> = BTreeSet::new();
    let entity_keys: Vec<RowKey> = touched_entities.iter().map(|e| key(Table::Entity, e)).collect();
    for row in snap.get_many(&entity_keys)?.into_iter().flatten() {
        let Row::Entity(e) = row else { continue };
        let left = snap
            .assertions_about(&e.subjects())?
            .into_iter()
            .any(|a| !gone_claim_set.contains(&a.header.id));
        if left {
            rewritten.insert(key(Table::Entity, &e.header.id));
        } else {
            removed.insert(key(Table::Entity, &e.header.id));
        }
    }
    // Claims about a removed entity have nothing to be about.
    let removed_entities: Vec<ObjectId> = removed.iter().filter(|k| k.table == Table::Entity).map(|k| k.id.clone()).collect();
    if !removed_entities.is_empty() {
        let subjects: Vec<String> = removed_entities.iter().map(entity_subject).collect();
        for a in snap.assertions_about(&subjects)? {
            removed.insert(key(Table::Assertion, &a.header.id));
            flagged_claims.retain(|c| c != &a.header.id);
        }
    }

    // Edges to anything that goes, apart from a page's own links, which stay and show as broken.
    let mut ends: Vec<ObjectId> = removed.iter().filter(|k| k.table != Table::Evidence).map(|k| k.id.clone()).collect();
    ends.sort();
    ends.dedup();
    let mut flagged_pages: BTreeSet<ObjectId> = BTreeSet::new();
    for chunk in ends.chunks(PAGE_LIMIT) {
        for e in snap.edges_of(chunk)? {
            if page_edge(&e) {
                flagged_pages.insert(e.from.clone());
            } else {
                removed.insert(key(Table::Edge, &e.header.id));
            }
        }
    }
    // Any other entity whose row holds a claim or an entity that goes is worked out again too, such as one another was merged into, whose fields come from the merged one's claims.
    let so_far = Removed::new(removed.iter().cloned());
    all_rows(snap, Table::Entity, |r| {
        if let Row::Entity(e) = r
            && !so_far.has(Table::Entity, &e.header.id)
            && rewrite_entity(&e, &so_far) != e
        {
            rewritten.insert(key(Table::Entity, &e.header.id));
        }
    })?;

    // ChangeSets keep what didn't come from the document, and go if nothing's left.
    let so_far = Removed::new(removed.iter().cloned());
    let mut gone_changesets = vec![];
    all_rows(snap, Table::ChangeSet, |r| {
        if let Row::ChangeSet(c) = r {
            match rewrite_changeset(&c, &so_far) {
                None => gone_changesets.push(c.header.id.clone()),
                Some(new) if new != c => {
                    rewritten.insert(key(Table::ChangeSet, &c.header.id));
                }
                Some(_) => {}
            }
        }
    })?;
    for c in &gone_changesets {
        removed.insert(key(Table::ChangeSet, c));
        // A reviewer's verdict has its ChangeSet's id.
        removed.insert(key(Table::ReviewVerdict, c));
    }
    let verdict_keys: Vec<RowKey> = removed.iter().filter(|k| k.table == Table::ReviewVerdict).cloned().collect();
    for chunk in verdict_keys.chunks(PAGE_LIMIT) {
        for (k, row) in chunk.iter().zip(snap.get_many(chunk)?) {
            if row.is_none() {
                removed.remove(k);
            }
        }
    }

    // Audits of claims that go, and what a chat run read.
    let so_far = Removed::new(removed.iter().cloned());
    all_rows(snap, Table::AuditCheck, |r| {
        if let Row::AuditCheck(c) = r
            && (so_far.has(Table::Assertion, &c.claim_id) || c.changeset_id.as_ref().is_some_and(|id| so_far.has(Table::ChangeSet, id)))
        {
            removed.insert(key(Table::AuditCheck, &c.header.id));
        }
    })?;
    all_rows(snap, Table::AuditPlanted, |r| {
        if so_far.has(Table::Assertion, &r.header().id) {
            removed.insert(key(Table::AuditPlanted, &r.header().id));
        }
    })?;
    all_rows(snap, Table::Derivation, |r| {
        if let Row::Derivation(d) = r {
            if sources.contains(&d.source_id) {
                removed.insert(key(Table::Derivation, &d.header.id));
            } else if rewrite_derivation(&d, &so_far) != d {
                rewritten.insert(key(Table::Derivation, &d.header.id));
            }
        }
    })?;

    // Jobs about anything that goes. An embedding job keeps the rest of its batch.
    let so_far = Removed::new(removed.iter().cloned());
    let mut gone_jobs = vec![];
    all_jobs(snap, |j| {
        if j.kind == crate::EMBED_JOB {
            match rewrite_job(&j, &so_far) {
                None => gone_jobs.push(j.header.id.clone()),
                Some(new) if new != j => {
                    rewritten.insert(key(Table::Job, &j.header.id));
                }
                Some(_) => {}
            }
        } else if so_far.mentioned_in(&j.input) {
            gone_jobs.push(j.header.id.clone());
        }
    })?;
    removed.extend(gone_jobs.iter().map(|j| key(Table::Job, j)));

    // Texts whose embeddings go: those of rows that go, and the old text of rows that change.
    let mut docs = vec![];
    let segs: Vec<ObjectId> = segments.clone();
    for chunk in segs.chunks(PAGE_LIMIT) {
        docs.extend(crate::lexical::segment_ops(snap, chunk)?);
    }
    let claim_ids: Vec<ObjectId> = removed
        .iter()
        .filter(|k| k.table == Table::Assertion)
        .map(|k| k.id.clone())
        .chain(flagged_claims.iter().cloned())
        .collect();
    for chunk in claim_ids.chunks(PAGE_LIMIT) {
        docs.extend(crate::lexical::assertion_ops(snap, chunk)?);
    }
    let entity_ids: Vec<ObjectId> = removed
        .iter()
        .chain(rewritten.iter())
        .filter(|k| k.table == Table::Entity)
        .map(|k| k.id.clone())
        .collect();
    for chunk in entity_ids.chunks(PAGE_LIMIT) {
        docs.extend(crate::entity::entity_ops(snap, chunk)?);
    }
    let upserts: Vec<memex_types::IndexDoc> = docs
        .into_iter()
        .filter_map(|op| match op {
            memex_types::IndexOp::Upsert(d) => Some(d),
            memex_types::IndexOp::Delete(_) => None,
        })
        .collect();
    let embeddings: BTreeSet<String> = crate::vector::embeddable(snap, upserts)?.into_iter().map(|d| d.hash).collect();

    let record = ErasureRecord {
        changeset_id: ObjectId::default(),
        logical_id: named.logical_id.clone(),
        sources,
        removed: removed.into_iter().collect(),
        rewritten: rewritten.into_iter().collect(),
        flagged_claims: flagged_claims.into_iter().collect::<BTreeSet<_>>().into_iter().collect(),
        flagged_pages: flagged_pages.into_iter().collect(),
    };
    let blobs = blob_candidates.into_iter().filter(|b| !b.is_empty()).collect();
    Ok(Some(ErasurePlan {
        record,
        blobs,
        embeddings: embeddings.into_iter().collect(),
    }))
}

/// Blobs among these that no row still refers to, once the erasure has committed.
pub fn unreferenced_blobs<S: Snapshot>(snap: &S, candidates: &[String]) -> Result<Vec<String>, StoreError> {
    let mut left: BTreeSet<String> = candidates.iter().cloned().collect();
    for hash in candidates {
        if snap.source_by_hash(hash)?.is_some() {
            left.remove(hash);
        }
    }
    if left.is_empty() {
        return Ok(vec![]);
    }
    all_rows(snap, Table::Rendition, |r| {
        if let Row::Rendition(r) = r {
            left.remove(&r.text_blob);
            left.remove(&r.words_blob);
        }
    })?;
    Ok(left.into_iter().collect())
}

/// Every blob a row refers to, for the orphan sweep.
pub fn referenced_blobs<S: Snapshot>(snap: &S) -> Result<HashSet<String>, StoreError> {
    let mut out = HashSet::new();
    all_rows(snap, Table::Source, |r| {
        if let Row::Source(s) = r {
            out.insert(s.sha256);
        }
    })?;
    all_rows(snap, Table::Rendition, |r| {
        if let Row::Rendition(r) = r {
            out.insert(r.text_blob);
            out.insert(r.words_blob);
        }
    })?;
    Ok(out)
}

fn rewrite_evidence(evidence: &[(ObjectId, EvidenceInput)], removed: &Removed) -> Vec<(ObjectId, EvidenceInput)> {
    evidence
        .iter()
        .filter(|(id, e)| !removed.has(Table::Evidence, id) && !removed.has(Table::Segment, &e.segment_id))
        .cloned()
        .collect()
}

/// An op with what came from the removed rows taken out, or none if nothing of it is left.
pub fn rewrite_op(op: &Op, removed: &Removed) -> Option<Op> {
    let gone = |t: Table, id: &ObjectId| removed.has(t, id);
    match op {
        Op::AddAssertion { id, subject, evidence, .. } => {
            if gone(Table::Assertion, id) || entity_of_subject(subject).is_some_and(|e| gone(Table::Entity, &e)) {
                return None;
            }
            let kept = rewrite_evidence(evidence, removed);
            if !evidence.is_empty() && kept.is_empty() {
                return None;
            }
            let mut out = op.clone();
            if let Op::AddAssertion { evidence, .. } = &mut out {
                *evidence = kept;
            }
            Some(out)
        }
        Op::SupersedeAssertion {
            target,
            base_version,
            replacement,
        } => rewrite_op(replacement, removed).map(|r| Op::SupersedeAssertion {
            target: target.clone(),
            base_version: *base_version,
            replacement: Box::new(r),
        }),
        Op::RetractAssertion { target, .. } => (!gone(Table::Assertion, target)).then(|| op.clone()),
        Op::AddEdge { id, from, to, .. } => (!gone(Table::Edge, id) && !removed.ids.contains(from) && !removed.ids.contains(to)).then(|| op.clone()),
        Op::RemoveEdge { id, .. } => (!gone(Table::Edge, id)).then(|| op.clone()),
        Op::SetSourceTrust { id, .. } | Op::SetSourceMetadata { id, .. } => (!gone(Table::Source, id)).then(|| op.clone()),
        Op::CreateEntity { id, .. } | Op::SetEntityAliases { id, .. } => (!gone(Table::Entity, id)).then(|| op.clone()),
        Op::MergeEntities { source, target, .. } => (!gone(Table::Entity, source) && !gone(Table::Entity, target)).then(|| op.clone()),
        Op::SplitEntity {
            id,
            base_version,
            new_id,
            moved,
            aliases,
        } => (!gone(Table::Entity, id) && !gone(Table::Entity, new_id)).then(|| Op::SplitEntity {
            id: id.clone(),
            base_version: *base_version,
            new_id: new_id.clone(),
            moved: moved.iter().filter(|m| !gone(Table::Assertion, &m.assertion)).cloned().collect(),
            aliases: aliases.clone(),
        }),
        // A person's pages stay as written, and an erasure is the record of itself.
        Op::CreatePage { .. }
        | Op::EditPage { .. }
        | Op::DeletePage { .. }
        | Op::RenamePage { .. }
        | Op::RevertPage { .. }
        | Op::DefineEntityType { .. }
        | Op::EraseSource { .. } => Some(op.clone()),
    }
}

/// A ChangeSet with what came from the removed rows taken out, or none if it was only about them.
pub fn rewrite_changeset(c: &ChangeSetRow, removed: &Removed) -> Option<ChangeSetRow> {
    if removed.has(Table::ChangeSet, &c.header.id) {
        return None;
    }
    let ops: Vec<Op> = c.ops.iter().filter_map(|op| rewrite_op(op, removed)).collect();
    if ops.is_empty() && !c.ops.is_empty() {
        return None;
    }
    let mut out = c.clone();
    let changed = ops != c.ops;
    out.ops = ops;
    out.original_ops = c.original_ops.as_ref().map(|o| o.iter().filter_map(|op| rewrite_op(op, removed)).collect());
    out.applied = c.applied.iter().filter_map(|m| rewrite_mutation(m, removed)).collect();
    out.inverse = c.inverse.iter().filter_map(|m| rewrite_mutation(m, removed)).collect();
    if let Some(origin) = &mut out.origin {
        origin.source_ids.retain(|s| !removed.has(Table::Source, s));
        origin.rendition_ids.retain(|r| !removed.has(Table::Rendition, r));
    }
    // A validator's detail or a refusal's reason can quote what was proposed.
    if changed || out.original_ops != c.original_ops {
        for v in &mut out.validators {
            v.detail = None;
        }
        if out.decided_by.as_deref() == Some(crate::review::VALIDATORS) {
            out.decision_reason = None;
        }
    }
    Some(out)
}

/// An entity without the removed claims and entities in it. Its fields are worked out again from its claims by the erasure's reducer.
pub fn rewrite_entity(e: &EntityRow, removed: &Removed) -> EntityRow {
    let mut out = e.clone();
    out.fields.retain(|f| !removed.has(Table::Assertion, &f.assertion_id));
    for f in &mut out.fields {
        f.others.retain(|o| !removed.has(Table::Assertion, &o.assertion_id));
    }
    out.merged_from.retain(|m| !removed.has(Table::Entity, m));
    if out.redirect_to.as_ref().is_some_and(|t| removed.has(Table::Entity, t)) {
        out.redirect_to = None;
    }
    out
}

/// A chat run's Derivation keeps its other ids. It holds no text, but nothing should point at what's gone.
pub fn rewrite_derivation(d: &DerivationRow, removed: &Removed) -> DerivationRow {
    let mut out = d.clone();
    out.segment_ids.retain(|s| !removed.has(Table::Segment, s));
    out
}

/// An embedding job keeps the rest of its batch, and goes if nothing is left. Other jobs are kept or removed whole by the plan.
pub fn rewrite_job(j: &JobRow, removed: &Removed) -> Option<JobRow> {
    if removed.has(Table::Job, &j.header.id) {
        return None;
    }
    if j.kind != crate::EMBED_JOB {
        return Some(j.clone());
    }
    let Ok(mut input) = serde_json::from_value::<crate::vector::EmbedInput>(j.input.clone()) else {
        return Some(j.clone());
    };
    let before = input.docs.len();
    input.docs.retain(|d| {
        let table = match d.kind {
            memex_types::DocKind::Segment => Table::Segment,
            memex_types::DocKind::Claim => Table::Assertion,
            memex_types::DocKind::Entity => Table::Entity,
            memex_types::DocKind::Page => Table::Page,
            memex_types::DocKind::Block => Table::Block,
        };
        !removed.has(table, &d.id)
    });
    if input.docs.len() == before {
        return Some(j.clone());
    }
    if input.docs.is_empty() {
        return None;
    }
    let mut out = j.clone();
    out.input = serde_json::to_value(input).unwrap_or(Value::Null);
    Some(out)
}

/// A row as the erasure leaves it, or none if it goes.
pub fn rewrite_row(row: &Row, removed: &Removed) -> Option<Row> {
    if removed.has_key(&row.key()) {
        return None;
    }
    Some(match row {
        Row::ChangeSet(c) => Row::ChangeSet(rewrite_changeset(c, removed)?),
        Row::Entity(e) => Row::Entity(rewrite_entity(e, removed)),
        Row::Derivation(d) => Row::Derivation(rewrite_derivation(d, removed)),
        Row::Job(j) => Row::Job(rewrite_job(j, removed)?),
        other => other.clone(),
    })
}

pub fn rewrite_mutation(m: &Mutation, removed: &Removed) -> Option<Mutation> {
    match m {
        Mutation::Put(row) => rewrite_row(row, removed).map(Mutation::Put),
        Mutation::Delete(k) => (!removed.has_key(k)).then(|| m.clone()),
    }
}

/// Whether an event's payload is the form an erasure leaves when the event's own kind can't hold what's left.
pub fn is_erased_form(payload: &Value) -> bool {
    payload.get("erased").and_then(Value::as_bool) == Some(true)
}

/// The rows an erased-form payload still writes.
pub fn erased_form_mutations(payload: &Value) -> Result<Vec<Mutation>, serde_json::Error> {
    serde_json::from_value(payload.get("mutations").cloned().unwrap_or_else(|| json!([])))
}

fn erased_form(mutations: &[Mutation]) -> Value {
    json!({"erased": true, "mutations": mutations})
}

/// The event's payload as the erasure leaves it, or none if it doesn't change. The envelope stays: sequence, actor, time, kind and version.
pub fn rewrite_event(e: &Event, removed: &Removed) -> Result<Option<Value>, String> {
    let rewrite_all = |ms: Vec<Mutation>| -> (Vec<Mutation>, bool) {
        let out: Vec<Mutation> = ms.iter().filter_map(|m| rewrite_mutation(m, removed)).collect();
        let changed = out != ms;
        (out, changed)
    };
    if is_erased_form(&e.payload) {
        let ms = erased_form_mutations(&e.payload).map_err(|err| format!("event {}: {err}", e.seq))?;
        let (out, changed) = rewrite_all(ms);
        return Ok(changed.then(|| erased_form(&out)));
    }
    let id_of = |p: &Value| p.get("id").and_then(Value::as_str).map(|s| ObjectId(s.into()));
    match e.kind.as_str() {
        "state.recorded" => {
            let p: crate::StateRecorded = serde_json::from_value(e.payload.clone()).map_err(|err| format!("event {}: {err}", e.seq))?;
            let (out, changed) = rewrite_all(p.mutations);
            Ok(changed.then(|| json!({"mutations": out})))
        }
        "changeset.recorded" => {
            let p: crate::ChangeSetRecorded = serde_json::from_value(e.payload.clone()).map_err(|err| format!("event {}: {err}", e.seq))?;
            let (out, changed) = rewrite_all(p.mutations);
            match rewrite_changeset(&p.changeset, removed) {
                None => Ok(Some(erased_form(&out))),
                Some(c) if c == p.changeset && !changed => Ok(None),
                Some(c) => Ok(Some(
                    serde_json::to_value(crate::ChangeSetRecorded { changeset: c, mutations: out }).map_err(|err| err.to_string())?,
                )),
            }
        }
        "source.recorded" => Ok(id_of(&e.payload).filter(|id| removed.has(Table::Source, id)).map(|_| erased_form(&[]))),
        "segment.recorded" => Ok(id_of(&e.payload).filter(|id| removed.has(Table::Segment, id)).map(|_| erased_form(&[]))),
        "edge.linked" | "edge.unlinked" => Ok(id_of(&e.payload).filter(|id| removed.has(Table::Edge, id)).map(|_| erased_form(&[]))),
        _ => Ok(None),
    }
}

/// The `source.erased` reducer: removes what's still there, rewrites what's listed, and works the listed entities out again from the claims left. The same mutations come out whether the rows it removes are there or a rewritten log never made them, which is what lets the log replay to the same state.
pub fn reduce_erased<S: Snapshot>(snap: &S, e: &Event) -> Result<Vec<Mutation>, crate::ReduceError> {
    let record: ErasureRecord = serde_json::from_value(e.payload.clone()).map_err(|err| crate::ReduceError::Corrupt {
        seq: e.seq,
        detail: err.to_string(),
    })?;
    let removed = Removed::of(&record);
    let mut out = vec![];
    for chunk in record.removed.chunks(PAGE_LIMIT) {
        for (k, row) in chunk.iter().zip(snap.get_many(chunk)?) {
            if row.is_some() {
                out.push(Mutation::Delete(k.clone()));
            }
        }
    }
    let mut entities = vec![];
    for chunk in record.rewritten.chunks(PAGE_LIMIT) {
        for row in snap.get_many(chunk)?.into_iter().flatten() {
            match row {
                Row::Entity(en) => entities.push(rewrite_entity(&en, &removed)),
                other => {
                    if let Some(new) = rewrite_row(&other, &removed) {
                        out.push(Mutation::Put(new));
                    }
                }
            }
        }
    }
    if !entities.is_empty() {
        let names: Vec<String> = entities.iter().map(|en| en.type_name.clone()).collect::<BTreeSet<_>>().into_iter().collect();
        let types = crate::entity::latest_types(snap, &names)?;
        let subjects: Vec<String> = entities.iter().flat_map(EntityRow::subjects).collect::<BTreeSet<_>>().into_iter().collect();
        let claims: Vec<memex_types::AssertionRow> = snap
            .assertions_about(&subjects)?
            .into_iter()
            .filter(|a| !removed.has(Table::Assertion, &a.header.id))
            .collect();
        for mut en in entities {
            if let Some(t) = types.get(&en.type_name) {
                let subjects = en.subjects();
                let mine: Vec<&memex_types::AssertionRow> = claims.iter().filter(|a| subjects.contains(&a.subject)).collect();
                en.type_version = t.version_no;
                crate::entity::materialize(&mut en, &t.schema, &mine);
            }
            en.header.version += 1;
            en.updated_at_ms = e.time_ms;
            out.push(Mutation::Put(Row::Entity(en)));
        }
    }
    Ok(out)
}

/// The event that records an erasure.
pub fn erased_event_payload(record: &ErasureRecord) -> Value {
    serde_json::to_value(record).unwrap_or(Value::Null)
}

pub fn is_erased_event(kind: &str) -> bool {
    kind == SOURCE_ERASED
}

#[cfg(test)]
mod tests {
    use super::*;
    use memex_types::{Header, Stance, Status};

    fn header(id: &str) -> Header {
        Header {
            id: ObjectId(id.into()),
            creator: "a".into(),
            status: Status::Active,
            version: 1,
            acl_id: None,
        }
    }

    fn removed(keys: &[(Table, &str)]) -> Removed {
        Removed::new(keys.iter().map(|(t, id)| key(*t, &ObjectId((*id).into()))))
    }

    fn claim(id: &str, segments: &[&str]) -> Op {
        Op::AddAssertion {
            id: ObjectId(id.into()),
            subject: "s".into(),
            predicate: "p".into(),
            value: json!("v"),
            epistemic: memex_types::Epistemic::Asserted,
            machine: true,
            confidence_permille: 0,
            valid_from_ms: None,
            valid_to_ms: None,
            evidence: segments
                .iter()
                .map(|s| {
                    (
                        ObjectId(format!("ev-{s}")),
                        EvidenceInput {
                            segment_id: ObjectId((*s).into()),
                            quote: format!("quote of {s}"),
                            stance: Stance::Supports,
                        },
                    )
                })
                .collect(),
        }
    }

    #[test]
    fn a_claim_loses_only_the_evidence_that_goes() {
        let r = removed(&[(Table::Segment, "s1")]);
        let Some(Op::AddAssertion { evidence, .. }) = rewrite_op(&claim("c", &["s1", "s2"]), &r) else {
            panic!()
        };
        assert_eq!(evidence.len(), 1);
        assert_eq!(evidence[0].1.segment_id.0, "s2");
        assert_eq!(rewrite_op(&claim("c", &["s1"]), &r), None, "nothing it stood on is left");
        assert_eq!(
            rewrite_op(&claim("c", &[]), &r),
            Some(claim("c", &[])),
            "a person's claim with no evidence isn't about the document"
        );
    }

    #[test]
    fn mentions_are_found_inside_prefixed_strings_and_nested_values() {
        let r = removed(&[(Table::Entity, "e1")]);
        assert!(r.mentioned_in(&json!({"a": [{"b": "entity:e1"}]})));
        assert!(!r.mentioned_in(&json!({"a": "e10"})));
    }

    #[test]
    fn a_changeset_only_about_the_document_goes_and_others_lose_their_quotes() {
        let r = removed(&[(Table::Segment, "s1"), (Table::Assertion, "c1")]);
        let mut c: ChangeSetRow = serde_json::from_value(json!({
            "header": header("cs"), "state": "pending", "proposer": "x", "proposer_is_machine": true, "rationale": "r", "idempotency_key": "k",
            "ops": [], "validators": [{"validator": "quote", "passed": false, "detail": "quote of s1 not found"}], "risk": "review", "proposed_at_ms": 0,
            "decided_by": null, "decided_at_ms": null, "decision_reason": null, "commit_seq": null, "applied": [], "inverse": [], "reverts": null, "reverted_by": null
        }))
        .unwrap();
        c.ops = vec![claim("c1", &["s1"])];
        assert_eq!(rewrite_changeset(&c, &r), None);
        c.ops = vec![claim("c1", &["s1"]), claim("c2", &["s1", "s2"])];
        let kept = rewrite_changeset(&c, &r).unwrap();
        assert_eq!(kept.ops.len(), 1);
        assert!(!serde_json::to_string(&kept).unwrap().contains("quote of s1"));
        assert_eq!(rewrite_changeset(&kept, &r).as_ref(), Some(&kept), "rewriting twice changes nothing");
    }

    #[test]
    fn events_keep_their_envelope_and_lose_what_goes() {
        let r = removed(&[(Table::Source, "src")]);
        let e = Event {
            seq: 3,
            vault: memex_types::VaultId("v".into()),
            actor: "a".into(),
            time_ms: 1,
            kind: "source.recorded".into(),
            schema_version: 3,
            payload: json!({"id": "src", "name": "secret.pdf"}),
        };
        let new = rewrite_event(&e, &r).unwrap().unwrap();
        assert!(is_erased_form(&new));
        assert!(!new.to_string().contains("secret"));
        let other = Event {
            payload: json!({"id": "other"}),
            ..e
        };
        assert_eq!(rewrite_event(&other, &r).unwrap(), None);
    }
}
