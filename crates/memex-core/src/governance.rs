//! The proposal flow's pure parts: validators, the risk table, planning ops into mutations, inverses and as-of reads.
use crate::{ChangeSetRecorded, Snapshot, StoreError};
use memex_types::{
    AssertionRow, EdgeRow, Epistemic, Event, EvidenceRow, Header, Mutation, ObjectId, Op, OpKind, RiskDecision, Row, RowKey, Status, Table, ValidatorResult,
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fmt::Write as _;

/// Who is asking, as far as validators and risk care.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Author {
    pub machine: bool,
}

/// The always-review rules, in one table. Merges, deletes, breaking schema changes, edits to verified claims, promoting untrusted sources and verifying what they say always get a human, and only a person can approve them.
pub const ALWAYS_REVIEW: [OpKind; 8] = [
    OpKind::Delete,
    OpKind::MergeEntities,
    OpKind::SplitEntity,
    OpKind::BreakingSchemaChange,
    OpKind::EditVerifiedClaim,
    OpKind::PromoteUntrustedSource,
    OpKind::RemoveLink,
    OpKind::ClaimVerified,
];

/// Pure. A machine's auto-commit is off unless spec 025's audit earned it, and self-reported confidence isn't an input here at all.
pub fn risk(kinds: &[OpKind], author: Author, machine_earned_auto_commit: bool, validators_passed: bool) -> RiskDecision {
    if !validators_passed {
        return RiskDecision::Reject;
    }
    if kinds.iter().any(|k| ALWAYS_REVIEW.contains(k)) {
        return RiskDecision::Review;
    }
    // An agent's type never goes live without review, however much auto-commit it has earned for claims.
    if author.machine && kinds.contains(&OpKind::AdditiveSchemaChange) {
        return RiskDecision::Review;
    }
    if author.machine && !machine_earned_auto_commit {
        return RiskDecision::Review;
    }
    RiskDecision::AutoCommit
}

/// Normalizes subject and predicate so trivial differences in case and spacing count as the same claim. The value stays exact.
pub fn canonical_hash(subject: &str, predicate: &str, value: &Value) -> String {
    let norm = |t: &str| t.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase();
    let text = format!("{}\u{1f}{}\u{1f}{value}", norm(subject), norm(predicate));
    let mut out = String::with_capacity(64);
    for b in Sha256::digest(text.as_bytes()) {
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// A quote must appear verbatim in its segment, apart from runs of whitespace, which PDF text and models both change.
pub fn quote_in_text(quote: &str, text: &str) -> bool {
    let norm = |t: &str| t.split_whitespace().collect::<Vec<_>>().join(" ");
    let q = norm(quote);
    !q.is_empty() && norm(text).contains(&q)
}

fn pass(name: &str) -> ValidatorResult {
    ValidatorResult {
        validator: name.into(),
        passed: true,
        detail: None,
    }
}

fn fail(name: &str, detail: String) -> ValidatorResult {
    ValidatorResult {
        validator: name.into(),
        passed: false,
        detail: Some(detail),
    }
}

fn claims(ops: &[Op]) -> Vec<&Op> {
    let mut out = vec![];
    for op in ops {
        match op {
            Op::AddAssertion { .. } => out.push(op),
            Op::SupersedeAssertion { replacement, .. } => out.push(replacement.as_ref()),
            _ => {}
        }
    }
    out
}

/// The shape of the request. Pure, with no reads.
pub fn validate_schema(ops: &[Op], rationale: &str, idempotency_key: &str) -> ValidatorResult {
    let name = "schema";
    if ops.is_empty() {
        return fail(name, "a ChangeSet needs at least one op".into());
    }
    if rationale.trim().is_empty() {
        return fail(name, "a rationale is required".into());
    }
    if idempotency_key.trim().is_empty() || idempotency_key.len() > 200 {
        return fail(name, "an idempotency key of 1 to 200 characters is required".into());
    }
    for op in ops {
        if let Op::SupersedeAssertion { replacement, .. } = op
            && !matches!(replacement.as_ref(), Op::AddAssertion { .. })
        {
            return fail(name, "an assertion can only be replaced by an assertion".into());
        }
    }
    for c in claims(ops) {
        if let Op::AddAssertion {
            subject,
            predicate,
            confidence_permille,
            ..
        } = c
        {
            if subject.trim().is_empty() || predicate.trim().is_empty() {
                return fail(name, "a claim needs a subject and a predicate".into());
            }
            if *confidence_permille > 1000 {
                return fail(name, format!("confidence {confidence_permille} is over 1000"));
            }
        }
    }
    pass(name)
}

/// Evidence must cite the document as it stands. A rendition that has been replaced says nothing about what the document says now, and its anchors wouldn't move with the rest. A segment that belongs to no rendition has nothing to go stale.
pub fn validate_current_rendition<S: Snapshot>(snap: &S, ops: &[Op]) -> Result<ValidatorResult, StoreError> {
    let name = "current_rendition";
    let places = crate::review::evidence_places(snap, ops)?;
    let keys: Vec<RowKey> = places
        .rendition_ids
        .iter()
        .map(|id| RowKey {
            table: Table::Rendition,
            id: id.clone(),
        })
        .collect();
    if keys.is_empty() {
        return Ok(pass(name));
    }
    for row in snap.get_many(&keys)?.into_iter().flatten() {
        if let Row::Rendition(r) = row
            && r.superseded_by.is_some()
        {
            return Ok(fail(
                name,
                format!("rendition {} has been replaced, as its document was processed again", r.header.id),
            ));
        }
    }
    Ok(pass(name))
}

/// A machine's claim must cite something. Pure.
pub fn validate_evidence_required(ops: &[Op]) -> ValidatorResult {
    let name = "evidence_required";
    for c in claims(ops) {
        if let Op::AddAssertion {
            machine: true,
            evidence,
            subject,
            predicate,
            ..
        } = c
            && evidence.is_empty()
        {
            return fail(name, format!("the machine claim {subject} {predicate} has no evidence"));
        }
    }
    pass(name)
}

/// Every quote must really be in its segment.
pub fn validate_quotes<S: Snapshot>(snap: &S, ops: &[Op]) -> Result<ValidatorResult, StoreError> {
    let name = "quote";
    let mut items = vec![];
    for c in claims(ops) {
        if let Op::AddAssertion { evidence, .. } = c {
            items.extend(evidence.iter().map(|(_, e)| e));
        }
    }
    let keys: Vec<RowKey> = items
        .iter()
        .map(|e| RowKey {
            table: Table::Segment,
            id: e.segment_id.clone(),
        })
        .collect();
    let rows = snap.get_many(&keys)?;
    for (e, row) in items.iter().zip(rows) {
        match row {
            Some(Row::Segment(seg)) => {
                if !quote_in_text(&e.quote, &seg.text) {
                    return Ok(fail(name, format!("the quote isn't in segment {}", e.segment_id)));
                }
            }
            _ => return Ok(fail(name, format!("segment {} doesn't exist", e.segment_id))),
        }
    }
    Ok(pass(name))
}

/// What dedup and contradiction found.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ClaimCheck {
    /// Pairs of a new claim's id and an existing active claim it disagrees with.
    pub contradictions: Vec<(ObjectId, ObjectId)>,
}

/// A claim identical to an active one is refused. One that disagrees is allowed, and gets linked, since contradictions are kept and surfaced.
pub fn validate_dedup<S: Snapshot>(snap: &S, ops: &[Op]) -> Result<(ValidatorResult, ClaimCheck), StoreError> {
    let name = "dedup";
    let mut new: Vec<(ObjectId, String, String, String)> = vec![];
    for c in claims(ops) {
        if let Op::AddAssertion {
            id, subject, predicate, value, ..
        } = c
        {
            new.push((id.clone(), subject.clone(), predicate.clone(), canonical_hash(subject, predicate, value)));
        }
    }
    let being_replaced: Vec<&ObjectId> = ops
        .iter()
        .filter_map(|o| if let Op::SupersedeAssertion { target, .. } = o { Some(target) } else { None })
        .collect();
    let hashes: Vec<String> = new.iter().map(|n| n.3.clone()).collect();
    let pairs: Vec<(String, String)> = new.iter().map(|n| (n.1.clone(), n.2.clone())).collect();
    let existing = snap.assertions_matching(&hashes, &pairs)?;
    // Two identical claims within one ChangeSet are as bad as one matching an existing claim.
    for (i, n) in new.iter().enumerate() {
        if new[..i].iter().any(|m| m.3 == n.3) {
            return Ok((fail(name, format!("claim {} appears twice in this ChangeSet", n.0)), ClaimCheck::default()));
        }
    }
    let mut check = ClaimCheck::default();
    for (id, subject, predicate, hash) in &new {
        for old in &existing {
            if being_replaced.contains(&&old.header.id) {
                continue;
            }
            if &old.canonical_hash == hash {
                return Ok((
                    fail(name, format!("claim {id} duplicates active claim {}", old.header.id)),
                    ClaimCheck::default(),
                ));
            }
            if old.subject == *subject && old.predicate == *predicate {
                check.contradictions.push((id.clone(), old.header.id.clone()));
            }
        }
    }
    Ok((pass(name), check))
}

/// Entity rules: a type's schema, and each value against its type. Nothing is written, as this only plans. Other reasons a plan fails are found when it runs for real, so a pending proposal can wait for them to change.
pub fn validate_entities<S: Snapshot>(snap: &S, ops: &[Op]) -> Result<ValidatorResult, StoreError> {
    let name = "entity";
    if !crate::entity::touches_entities(ops) {
        return Ok(pass(name));
    }
    let mut n = 0u64;
    let mut ids = || {
        n += 1;
        ObjectId(format!("validation-{n}"))
    };
    match plan(snap, ops, "validator", &ObjectId("validation".into()), 0, &[], &mut ids) {
        Err(PlanError::Invalid(detail)) => Ok(fail(name, detail)),
        Err(PlanError::Store(e)) => Err(e),
        _ => Ok(pass(name)),
    }
}

/// Page rules that a plan can check now: size, title, slug shape and a revision that exists. A slug another page holds or a stale version may change while a proposal waits, so those are found when it runs for real.
pub fn validate_pages<S: Snapshot>(snap: &S, ops: &[Op]) -> Result<ValidatorResult, StoreError> {
    let name = "page";
    if !crate::page::touches_pages(ops) {
        return Ok(pass(name));
    }
    let mut n = 0u64;
    let mut ids = || {
        n += 1;
        ObjectId(format!("validation-{n}"))
    };
    match crate::page::plan_pages(snap, ops, "validator", &ObjectId("validation".into()), 0, &mut ids) {
        Err(PlanError::Invalid(detail)) => Ok(fail(name, detail)),
        Err(PlanError::Store(e)) => Err(e),
        _ => Ok(pass(name)),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanError {
    /// A base version no longer matches. It carries the current row, for the caller to rebase on.
    Stale {
        key: RowKey,
        current: Box<Row>,
        base: u32,
    },
    Missing(RowKey),
    Exists(RowKey),
    /// Something only known at this point isn't allowed, such as a value its type refuses.
    Invalid(String),
    Store(StoreError),
}

impl From<StoreError> for PlanError {
    fn from(e: StoreError) -> Self {
        Self::Store(e)
    }
}

fn check_base(key: RowKey, row: Option<Row>, base: u32) -> Result<Row, PlanError> {
    let row = row.ok_or_else(|| PlanError::Missing(key.clone()))?;
    if row.header().version != base {
        return Err(PlanError::Stale {
            key,
            current: Box::new(row),
            base,
        });
    }
    Ok(row)
}

fn new_header(id: ObjectId, creator: &str) -> Header {
    Header {
        id,
        creator: creator.into(),
        status: Status::Active,
        version: 1,
        acl_id: None,
    }
}

fn assertion_rows(op: &Op, creator: &str, changeset: &ObjectId) -> Vec<Row> {
    let Op::AddAssertion {
        id,
        subject,
        predicate,
        value,
        epistemic,
        machine,
        confidence_permille,
        valid_from_ms,
        valid_to_ms,
        evidence,
    } = op
    else {
        return vec![];
    };
    let mut rows = vec![Row::Assertion(AssertionRow {
        header: new_header(id.clone(), creator),
        subject: subject.clone(),
        predicate: predicate.clone(),
        value: value.clone(),
        epistemic: *epistemic,
        machine: *machine,
        confidence_permille: *confidence_permille,
        valid_from_ms: *valid_from_ms,
        valid_to_ms: *valid_to_ms,
        canonical_hash: canonical_hash(subject, predicate, value),
        changeset_id: changeset.clone(),
        superseded_by: None,
    })];
    for (evidence_id, e) in evidence {
        rows.push(Row::Evidence(EvidenceRow {
            header: new_header(evidence_id.clone(), creator),
            assertion_id: id.clone(),
            segment_id: e.segment_id.clone(),
            quote: e.quote.clone(),
            stance: e.stance,
            anchor: None,
            anchor_broken: false,
        }));
    }
    rows
}

/// Turns ops into mutations against the current state. Base versions are checked again here, at commit, so a stale edit is caught however long review took.
pub fn plan<S: Snapshot>(
    snap: &S,
    ops: &[Op],
    creator: &str,
    changeset: &ObjectId,
    now_ms: i64,
    contradictions: &[(ObjectId, ObjectId)],
    edge_id: &mut dyn FnMut() -> ObjectId,
) -> Result<Vec<Mutation>, PlanError> {
    let mut out = vec![];
    let key = |table, id: &ObjectId| RowKey { table, id: id.clone() };
    let must_be_new = |k: RowKey| -> Result<(), PlanError> {
        match snap.get(&k)? {
            Some(_) => Err(PlanError::Exists(k)),
            None => Ok(()),
        }
    };
    for op in ops {
        match op {
            Op::AddAssertion { id, .. } => {
                must_be_new(key(Table::Assertion, id))?;
                out.extend(assertion_rows(op, creator, changeset).into_iter().map(Mutation::Put));
            }
            Op::SupersedeAssertion {
                target,
                base_version,
                replacement,
            } => {
                let k = key(Table::Assertion, target);
                let row = check_base(k.clone(), snap.get(&k)?, *base_version)?;
                let Row::Assertion(mut old) = row else { return Err(PlanError::Missing(k)) };
                let Op::AddAssertion { id: new_id, .. } = replacement.as_ref() else {
                    return Err(PlanError::Missing(k));
                };
                must_be_new(key(Table::Assertion, new_id))?;
                old.header.status = Status::Superseded;
                old.header.version += 1;
                old.superseded_by = Some(new_id.clone());
                out.push(Mutation::Put(Row::Assertion(old)));
                out.extend(assertion_rows(replacement, creator, changeset).into_iter().map(Mutation::Put));
            }
            Op::AddEdge { id, from, to, kind } => {
                must_be_new(key(Table::Edge, id))?;
                out.push(Mutation::Put(Row::Edge(EdgeRow {
                    header: new_header(id.clone(), creator),
                    from: from.clone(),
                    to: to.clone(),
                    kind: kind.clone(),
                    block_id: None,
                })));
            }
            Op::RemoveEdge { id, base_version } => {
                let k = key(Table::Edge, id);
                check_base(k.clone(), snap.get(&k)?, *base_version)?;
                out.push(Mutation::Delete(k));
            }
            Op::SetSourceTrust { id, base_version, trust_tier } => {
                let k = key(Table::Source, id);
                let row = check_base(k.clone(), snap.get(&k)?, *base_version)?;
                let Row::Source(mut s) = row else { return Err(PlanError::Missing(k)) };
                s.header.version += 1;
                s.trust_tier = trust_tier.clone();
                out.push(Mutation::Put(Row::Source(s)));
            }
            Op::SetSourceMetadata { id, base_version, metadata } => {
                let k = key(Table::Source, id);
                let row = check_base(k.clone(), snap.get(&k)?, *base_version)?;
                let Row::Source(mut s) = row else { return Err(PlanError::Missing(k)) };
                s.header.version += 1;
                s.metadata = metadata.clone();
                out.push(Mutation::Put(Row::Source(s)));
            }
            // Planned together below, since they depend on each other and on the claims above.
            Op::CreatePage { .. } | Op::EditPage { .. } | Op::DeletePage { .. } | Op::RenamePage { .. } | Op::RevertPage { .. } => {}
            Op::DefineEntityType { .. } | Op::CreateEntity { .. } | Op::SetEntityAliases { .. } | Op::MergeEntities { .. } | Op::SplitEntity { .. } => {}
        }
    }
    let entities = crate::entity::plan_entities(snap, ops, &out, creator, changeset, now_ms, edge_id)?;
    out.extend(entities);
    let pages = crate::page::plan_pages(snap, ops, creator, changeset, now_ms, edge_id)?;
    out.extend(pages);
    // A contradiction is kept and linked, so both claims stay visible.
    for (new_claim, old_claim) in contradictions {
        out.push(Mutation::Put(Row::Edge(EdgeRow {
            header: new_header(edge_id(), creator),
            from: new_claim.clone(),
            to: old_claim.clone(),
            kind: CONTRADICTS.into(),
            block_id: None,
        })));
    }
    Ok(out)
}

pub const CONTRADICTS: &str = "contradicts";

/// The inverse: for each row touched, put back what was there, or delete what wasn't. In reverse order.
pub fn inverse_of<S: Snapshot>(snap: &S, mutations: &[Mutation]) -> Result<Vec<Mutation>, StoreError> {
    let keys: Vec<RowKey> = mutations
        .iter()
        .map(|m| match m {
            Mutation::Put(r) => r.key(),
            Mutation::Delete(k) => k.clone(),
        })
        .collect();
    let before = snap.get_many(&keys)?;
    // Only the first touch of a key says what the row was before the whole set.
    let mut seen: HashMap<RowKey, ()> = HashMap::new();
    let mut out = vec![];
    for (key, row) in keys.into_iter().zip(before) {
        if seen.insert(key.clone(), ()).is_some() {
            continue;
        }
        out.push(match row {
            Some(r) => Mutation::Put(r),
            None => Mutation::Delete(key),
        });
    }
    out.reverse();
    Ok(out)
}

/// Why a committed ChangeSet can't be reverted any more.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NotReversible {
    AlreadyReverted,
    NotCommitted,
    /// A row it touched has changed since, so restoring it would lose that change.
    ChangedSince {
        key: RowKey,
    },
}

/// Pure given the snapshot. Safe only while every touched row is still exactly as this ChangeSet left it.
pub fn check_reversible<S: Snapshot>(snap: &S, cs: &memex_types::ChangeSetRow) -> Result<Result<(), NotReversible>, StoreError> {
    if cs.reverted_by.is_some() {
        return Ok(Err(NotReversible::AlreadyReverted));
    }
    if cs.state != memex_types::ChangeSetState::Committed {
        return Ok(Err(NotReversible::NotCommitted));
    }
    let mut last: HashMap<RowKey, Option<&Row>> = HashMap::new();
    for m in &cs.applied {
        match m {
            Mutation::Put(r) => last.insert(r.key(), Some(r)),
            Mutation::Delete(k) => last.insert(k.clone(), None),
        };
    }
    let keys: Vec<RowKey> = last.keys().cloned().collect();
    let rows = snap.get_many(&keys)?;
    for (key, now) in keys.into_iter().zip(rows) {
        if now.as_ref() != last[&key] {
            return Ok(Err(NotReversible::ChangedSince { key }));
        }
    }
    Ok(Ok(()))
}

/// The op kinds in a ChangeSet, looking at current state where the kind depends on it.
pub fn kinds_of<S: Snapshot>(snap: &S, ops: &[Op]) -> Result<Vec<OpKind>, StoreError> {
    let mut out = vec![];
    for op in ops {
        out.push(match op {
            Op::AddAssertion { .. } => OpKind::AddClaim,
            Op::SupersedeAssertion { target, .. } => match snap.get(&RowKey {
                table: Table::Assertion,
                id: target.clone(),
            })? {
                Some(Row::Assertion(a)) if a.epistemic == Epistemic::Verified => OpKind::EditVerifiedClaim,
                _ => OpKind::EditClaim,
            },
            Op::CreatePage { .. } => OpKind::CreatePage,
            Op::EditPage { .. } | Op::RenamePage { .. } | Op::RevertPage { .. } => OpKind::EditPage,
            Op::DeletePage { .. } => OpKind::Delete,
            Op::AddEdge { .. } => OpKind::AddLink,
            Op::RemoveEdge { .. } => OpKind::RemoveLink,
            // Raising trust is what the always-review rule is about. Lowering it is safe.
            Op::SetSourceTrust { id, trust_tier, .. } => match snap.get(&RowKey {
                table: Table::Source,
                id: id.clone(),
            })? {
                Some(Row::Source(s)) if s.trust_tier == crate::UNTRUSTED && trust_tier != crate::UNTRUSTED => OpKind::PromoteUntrustedSource,
                _ => OpKind::EditSource,
            },
            Op::SetSourceMetadata { .. } => OpKind::EditSource,
            Op::DefineEntityType { name, schema, .. } => crate::entity::schema_op_kind(snap, name, schema)?,
            Op::CreateEntity { .. } => OpKind::CreateEntity,
            Op::SetEntityAliases { .. } => OpKind::EditEntity,
            Op::MergeEntities { .. } => OpKind::MergeEntities,
            Op::SplitEntity { .. } => OpKind::SplitEntity,
        });
        let claim = match op {
            Op::SupersedeAssertion { replacement, .. } => replacement.as_ref(),
            other => other,
        };
        if crate::review::verified_without_standing(snap, claim)? {
            out.push(OpKind::ClaimVerified);
        }
    }
    Ok(out)
}

/// What the vault believed about a subject after event `seq`, computed from the log, so a later revert can't rewrite what was believed then. Pure.
pub fn believed_at(events: &[Event], seq: u64, subject: &str) -> Vec<AssertionRow> {
    let mut rows: HashMap<ObjectId, AssertionRow> = HashMap::new();
    for e in events.iter().filter(|e| e.seq <= seq && e.kind == "changeset.recorded") {
        let Ok(rec) = serde_json::from_value::<ChangeSetRecorded>(e.payload.clone()) else {
            continue;
        };
        for m in &rec.mutations {
            match m {
                Mutation::Put(Row::Assertion(a)) => {
                    rows.insert(a.header.id.clone(), a.clone());
                }
                Mutation::Delete(k) if k.table == Table::Assertion => {
                    rows.remove(&k.id);
                }
                _ => {}
            }
        }
    }
    let mut out: Vec<AssertionRow> = rows
        .into_values()
        .filter(|a| a.subject == subject && a.header.status == Status::Active)
        .collect();
    out.sort_by(|a, b| a.header.id.cmp(&b.header.id));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const HUMAN: Author = Author { machine: false };
    const MACHINE: Author = Author { machine: true };

    #[test]
    fn every_always_review_kind_goes_to_review_for_anyone_even_with_earned_auto_commit() {
        for kind in ALWAYS_REVIEW {
            for author in [HUMAN, MACHINE] {
                for earned in [false, true] {
                    assert_eq!(risk(&[kind], author, earned, true), RiskDecision::Review, "{kind:?}");
                    assert_eq!(risk(&[OpKind::AddClaim, kind], author, earned, true), RiskDecision::Review, "{kind:?} mixed");
                }
            }
        }
    }

    #[test]
    fn a_machine_only_auto_commits_when_earned() {
        assert_eq!(risk(&[OpKind::AddClaim], MACHINE, false, true), RiskDecision::Review);
        assert_eq!(risk(&[OpKind::AddClaim], MACHINE, true, true), RiskDecision::AutoCommit);
    }

    #[test]
    fn a_person_commits_low_risk_directly() {
        assert_eq!(risk(&[OpKind::EditPage], HUMAN, false, true), RiskDecision::AutoCommit);
    }

    #[test]
    fn failed_validators_always_reject() {
        for kind in ALWAYS_REVIEW {
            assert_eq!(risk(&[kind], HUMAN, true, false), RiskDecision::Reject);
        }
        assert_eq!(risk(&[OpKind::AddClaim], HUMAN, true, false), RiskDecision::Reject);
    }

    #[test]
    fn canonical_hash_ignores_case_and_spacing_but_not_value() {
        let a = canonical_hash("Acme  Pump", "weighs", &json!(12));
        assert_eq!(a, canonical_hash("acme pump", " WEIGHS ", &json!(12)));
        assert_ne!(a, canonical_hash("acme pump", "weighs", &json!(13)));
        assert_ne!(a, canonical_hash("acme pumps", "weighs", &json!(12)));
    }

    #[test]
    fn quotes_must_match_apart_from_whitespace() {
        assert!(quote_in_text("Göta  älv", "Staden ligger vid Göta\nälv."));
        assert!(!quote_in_text("Göta alv", "Staden ligger vid Göta älv."));
        assert!(!quote_in_text("", "any"));
        assert!(!quote_in_text("   ", "any"));
    }

    #[test]
    fn schema_refuses_bad_requests() {
        let page = Op::CreatePage {
            id: ObjectId("p".into()),
            title: "t".into(),
            body: "b".into(),
            slug: None,
            redirects: vec![],
        };
        assert!(validate_schema(std::slice::from_ref(&page), "why", "k").passed);
        assert!(!validate_schema(&[], "why", "k").passed);
        assert!(!validate_schema(std::slice::from_ref(&page), " ", "k").passed);
        assert!(!validate_schema(std::slice::from_ref(&page), "why", "").passed);
        let nested = Op::SupersedeAssertion {
            target: ObjectId("a".into()),
            base_version: 1,
            replacement: Box::new(page),
        };
        assert!(!validate_schema(&[nested], "why", "k").passed);
    }

    #[test]
    fn a_machine_claim_needs_evidence_and_a_person_s_does_not() {
        let claim = |machine| Op::AddAssertion {
            id: ObjectId("a".into()),
            subject: "s".into(),
            predicate: "p".into(),
            value: json!(1),
            epistemic: Epistemic::Asserted,
            machine,
            confidence_permille: 1000,
            valid_from_ms: None,
            valid_to_ms: None,
            evidence: vec![],
        };
        assert!(!validate_evidence_required(&[claim(true)]).passed);
        assert!(validate_evidence_required(&[claim(false)]).passed);
    }
}
