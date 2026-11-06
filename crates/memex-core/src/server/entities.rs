//! The entity registry's use cases on the facade. Every write here builds ops and goes through `propose`, so it gets the same validators, risk table and review queue as anything else.
use super::{Author, Caller, Operation, Proposal, ProposalOutcome, Server, ServerEdges, ServerError};
use crate::entity::{self, FieldDef, TypePreview};
use crate::governance;
use crate::{LexicalIndex, PAGE_LIMIT, Snapshot, Store};
use memex_types::{
    AssertionRow, ChangeSetRow, DocKind, EntityRow, EntityTypeRow, Epistemic, EvidenceInput, EvidenceRow, LexicalFilter, Mutation, ObjectId, Op, PrincipalKind,
    RiskDecision, Row, RowKey, SegmentRow, SharedValue, SourceRow, Status, Table, VaultId, entity_subject,
};
use serde_json::Value;
use std::collections::{BTreeSet, HashMap};
use std::sync::atomic::Ordering;

/// One field value being proposed, with what it rests on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldInput {
    pub field: String,
    pub value: Value,
    pub evidence: Vec<EvidenceInput>,
    /// The active claim this replaces, and the version it was read at. Without it the value is added beside any others, which is how a conflict stays visible.
    pub replaces: Option<(ObjectId, u32)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewEntity {
    pub type_name: String,
    pub aliases: Vec<String>,
    pub fields: Vec<FieldInput>,
    pub rationale: String,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityEdit {
    pub base_version: u32,
    /// Replaces the aliases. None leaves them.
    pub aliases: Option<Vec<String>>,
    pub fields: Vec<FieldInput>,
    pub rationale: String,
    pub idempotency_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SplitRequest {
    pub base_version: u32,
    /// The active claims that move to the new entity.
    pub moved: Vec<ObjectId>,
    pub aliases: Vec<String>,
    /// Anything else the new entity needs, such as its own name.
    pub fields: Vec<FieldInput>,
    pub rationale: String,
    pub idempotency_key: String,
}

/// A piece of evidence with what's needed to reach its source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClaimEvidence {
    pub evidence: EvidenceRow,
    pub segment: Option<SegmentRow>,
    pub source: Option<SourceRow>,
}

/// A claim behind a value, current or not.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityClaim {
    pub assertion: AssertionRow,
    pub evidence: Vec<ClaimEvidence>,
}

/// An entity with everything behind its values. An old id gets the entity it now leads to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityView {
    pub entity: EntityRow,
    /// The id asked for, when it isn't the entity's own.
    pub redirected_from: Option<ObjectId>,
    pub claims: HashMap<ObjectId, EntityClaim>,
}

/// Where an id leads now. An id that was never merged leads to itself, through nothing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Redirect {
    pub id: ObjectId,
    pub current_id: ObjectId,
    pub chain: Vec<ObjectId>,
}

/// One entity a mention may mean.
#[derive(Debug, Clone, PartialEq)]
pub struct EntityCandidate {
    pub entity: EntityRow,
    /// The identity field or `$alias` that matched exactly, or none for a keyword match.
    pub matched_on: Option<String>,
    pub exact: bool,
    pub score: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EntityResolution {
    pub candidates: Vec<EntityCandidate>,
    /// `lexical`, if the keyword index couldn't answer, so only exact matches are here.
    pub degraded: Vec<String>,
}

/// A preview with what would happen to this caller's proposal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypeDecision {
    pub preview: TypePreview,
    pub risk: RiskDecision,
}

/// A type's revert is blocked while anything depends on that version.
pub(super) fn type_revert_blocked<S: Snapshot>(snap: &S, cs: &ChangeSetRow) -> Result<Option<String>, ServerError> {
    for m in &cs.applied {
        let Mutation::Put(Row::EntityType(t)) = m else { continue };
        let latest = entity::latest_types(snap, std::slice::from_ref(&t.name))?;
        if latest.get(&t.name).is_some_and(|l| l.version_no > t.version_no) {
            return Ok(Some(format!("{} has a newer version than {}", t.name, t.version_no)));
        }
        let mut after = None;
        loop {
            let page = snap.entities_of_type(&t.name, after.as_ref(), PAGE_LIMIT)?;
            let Some(last) = page.last() else { break };
            if page.iter().any(|e| e.type_version >= t.version_no) {
                return Ok(Some(format!("entities were written at version {} of {}", t.version_no, t.name)));
            }
            after = Some(last.header.id.clone());
        }
    }
    Ok(None)
}

fn claim_op(id: ObjectId, entity: &ObjectId, input: &FieldInput, new_evidence_id: &mut dyn FnMut() -> ObjectId) -> Op {
    let add = Op::AddAssertion {
        id,
        subject: entity_subject(entity),
        predicate: input.field.clone(),
        value: input.value.clone(),
        // Whether it's a machine's comes from who holds the token, which `propose` fills in. A reviewer makes a claim verified, not its author.
        epistemic: Epistemic::Asserted,
        machine: false,
        // Not used to choose between values, since a model reports its own.
        confidence_permille: 1000,
        valid_from_ms: None,
        valid_to_ms: None,
        evidence: input.evidence.iter().map(|e| (new_evidence_id(), e.clone())).collect(),
    };
    match &input.replaces {
        Some((target, base_version)) => Op::SupersedeAssertion {
            target: target.clone(),
            base_version: *base_version,
            replacement: Box::new(add),
        },
        None => add,
    }
}

impl<E: ServerEdges> Server<E> {
    fn read_access(&self, caller: &Caller, vault: &VaultId) -> Result<(), ServerError> {
        self.allow(caller, Operation::VaultContentRead(vault))?;
        self.vault_exists(vault)
    }

    /// Refuses an agent, whatever its scope, on what's reserved for a person.
    pub(super) fn only_a_person<S: Snapshot>(&self, caller: &Caller, snap: &S, cs: &ChangeSetRow) -> Result<(), ServerError> {
        if caller.principal.kind == PrincipalKind::Human {
            return Ok(());
        }
        let kinds = governance::kinds_of(snap, &cs.ops)?;
        if entity::needs_a_person(&kinds, &cs.ops, cs.proposer_is_machine) {
            return Err(ServerError::Forbidden);
        }
        Ok(())
    }

    pub fn list_entity_types(&self, caller: &Caller, vault: &VaultId, after: Option<&str>, limit: usize) -> Result<Vec<EntityTypeRow>, ServerError> {
        self.read_access(caller, vault)?;
        let snap = self.engine.store().snapshot(vault)?;
        Ok(entity::list_types(&snap, after, limit)?)
    }

    /// Oldest first. Not found if there's no such type.
    pub fn entity_type_history(&self, caller: &Caller, vault: &VaultId, name: &str) -> Result<Vec<EntityTypeRow>, ServerError> {
        self.read_access(caller, vault)?;
        let snap = self.engine.store().snapshot(vault)?;
        let history = entity::type_history(&snap, name)?;
        if history.is_empty() {
            return Err(ServerError::NotFound);
        }
        Ok(history)
    }

    /// What submitting this schema would do, for this caller. It runs the same functions as a submitted change.
    pub fn preview_entity_type(&self, caller: &Caller, vault: &VaultId, name: &str, schema: &Value) -> Result<TypeDecision, ServerError> {
        self.allow(caller, Operation::VaultContentPropose(vault))?;
        self.vault_exists(vault)?;
        let snap = self.engine.store().snapshot(vault)?;
        let preview = entity::preview_change(&snap, name, schema)?;
        let kind = entity::schema_op_kind(&snap, name, schema)?;
        let machine = caller.principal.kind == PrincipalKind::Agent;
        let risk = governance::risk(
            &[kind],
            Author { machine },
            self.machine_auto_commit.load(Ordering::SeqCst),
            preview.problem.is_none(),
        );
        Ok(TypeDecision { preview, risk })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn propose_entity_type(
        &self,
        caller: &Caller,
        vault: &VaultId,
        name: &str,
        base_version: Option<u32>,
        schema: Value,
        rationale: String,
        idempotency_key: String,
    ) -> Result<ProposalOutcome, ServerError> {
        self.propose(
            caller,
            vault,
            Proposal {
                ops: vec![Op::DefineEntityType {
                    name: name.to_string(),
                    base_version,
                    schema,
                }],
                rationale,
                idempotency_key,
            },
            false,
        )
    }

    fn field_ops(&self, entity: &ObjectId, fields: &[FieldInput]) -> Vec<Op> {
        fields
            .iter()
            .map(|f| claim_op(ObjectId(self.new_id()), entity, f, &mut || ObjectId(self.new_id())))
            .collect()
    }

    pub fn propose_new_entity(&self, caller: &Caller, vault: &VaultId, new: NewEntity, dry_run: bool) -> Result<(ObjectId, ProposalOutcome), ServerError> {
        let id = ObjectId(self.new_id());
        let mut ops = vec![Op::CreateEntity {
            id: id.clone(),
            type_name: new.type_name,
            aliases: new.aliases,
        }];
        ops.extend(self.field_ops(&id, &new.fields));
        let outcome = self.propose(
            caller,
            vault,
            Proposal {
                ops,
                rationale: new.rationale,
                idempotency_key: new.idempotency_key,
            },
            dry_run,
        )?;
        // A retry gets the id its first attempt chose.
        let id = outcome
            .changeset
            .as_ref()
            .and_then(|c| {
                c.ops.iter().find_map(|op| match op {
                    Op::CreateEntity { id, .. } => Some(id.clone()),
                    _ => None,
                })
            })
            .unwrap_or(id);
        Ok((id, outcome))
    }

    pub fn propose_entity_edit(
        &self,
        caller: &Caller,
        vault: &VaultId,
        id: &ObjectId,
        edit: EntityEdit,
        dry_run: bool,
    ) -> Result<ProposalOutcome, ServerError> {
        self.read_access(caller, vault)?;
        let current = self.entity_row(vault, id)?.ok_or(ServerError::NotFound)?;
        if edit.base_version != current.header.version {
            return Err(ServerError::StaleVersion {
                key: current_key(id),
                base: edit.base_version,
                current: Box::new(Row::Entity(current)),
            });
        }
        let mut ops = self.field_ops(id, &edit.fields);
        if let Some(aliases) = edit.aliases {
            ops.push(Op::SetEntityAliases {
                id: id.clone(),
                base_version: edit.base_version,
                aliases,
            });
        }
        self.propose(
            caller,
            vault,
            Proposal {
                ops,
                rationale: edit.rationale,
                idempotency_key: edit.idempotency_key,
            },
            dry_run,
        )
    }

    /// Always goes to review, for anyone. The risk table sees to that.
    pub fn propose_merge(
        &self,
        caller: &Caller,
        vault: &VaultId,
        source: &ObjectId,
        target: &ObjectId,
        rationale: String,
        idempotency_key: String,
    ) -> Result<ProposalOutcome, ServerError> {
        self.read_access(caller, vault)?;
        let source_row = self.entity_row(vault, source)?.ok_or(ServerError::NotFound)?;
        let target_row = self.entity_row(vault, target)?.ok_or(ServerError::NotFound)?;
        self.propose(
            caller,
            vault,
            Proposal {
                ops: vec![Op::MergeEntities {
                    source: source.clone(),
                    source_version: source_row.header.version,
                    target: target.clone(),
                    target_version: target_row.header.version,
                }],
                rationale,
                idempotency_key,
            },
            false,
        )
    }

    /// Always goes to review, for anyone. The first result's id is the new entity's.
    pub fn propose_split(&self, caller: &Caller, vault: &VaultId, id: &ObjectId, split: SplitRequest) -> Result<(ObjectId, ProposalOutcome), ServerError> {
        self.read_access(caller, vault)?;
        let new_id = ObjectId(self.new_id());
        let mut ops = vec![Op::SplitEntity {
            id: id.clone(),
            base_version: split.base_version,
            new_id: new_id.clone(),
            moved: split
                .moved
                .into_iter()
                .map(|assertion| memex_types::MovedClaim {
                    assertion,
                    new_id: ObjectId(self.new_id()),
                })
                .collect(),
            aliases: split.aliases,
        }];
        ops.extend(self.field_ops(&new_id, &split.fields));
        let outcome = self.propose(
            caller,
            vault,
            Proposal {
                ops,
                rationale: split.rationale,
                idempotency_key: split.idempotency_key,
            },
            false,
        )?;
        let new_id = outcome
            .changeset
            .as_ref()
            .and_then(|c| {
                c.ops.iter().find_map(|op| match op {
                    Op::SplitEntity { new_id, .. } => Some(new_id.clone()),
                    _ => None,
                })
            })
            .unwrap_or(new_id);
        Ok((new_id, outcome))
    }

    fn entity_row(&self, vault: &VaultId, id: &ObjectId) -> Result<Option<EntityRow>, ServerError> {
        Ok(match self.engine.store().snapshot(vault)?.get(&current_key(id))? {
            Some(Row::Entity(e)) => Some(e),
            _ => None,
        })
    }

    /// Entities of a type in id order. `filters` holds (field, operator, value) in the type's terms, all of which must hold.
    pub fn list_entities(
        &self,
        caller: &Caller,
        vault: &VaultId,
        type_name: &str,
        filters: &[(String, String, String)],
        after: Option<&ObjectId>,
        limit: usize,
    ) -> Result<Vec<EntityRow>, ServerError> {
        self.read_access(caller, vault)?;
        let snap = self.engine.store().snapshot(vault)?;
        let types = entity::latest_types(&snap, &[type_name.to_string()])?;
        let t = types.get(type_name).ok_or(ServerError::NotFound)?;
        if filters.is_empty() {
            return Ok(snap.entities_of_type(type_name, after, limit)?);
        }
        let defs: Vec<FieldDef> = entity::fields_of(&t.schema);
        let parsed = filters
            .iter()
            .map(|(f, op, value)| entity::parse_filter(&defs, f, op, value))
            .collect::<Result<Vec<_>, String>>()
            .map_err(ServerError::Invalid)?;
        Ok(snap.entities_matching(type_name, &parsed, after, limit)?)
    }

    /// Follows merges. An old id answers with the entity it leads to, and says so.
    pub fn get_entity(&self, caller: &Caller, vault: &VaultId, id: &ObjectId) -> Result<EntityView, ServerError> {
        self.read_access(caller, vault)?;
        let snap = self.engine.store().snapshot(vault)?;
        let (entity, redirected_from) = {
            let (row, chain) = follow(&snap, id)?;
            (row, (!chain.is_empty()).then(|| id.clone()))
        };
        let ids: Vec<ObjectId> = entity
            .fields
            .iter()
            .flat_map(|f| std::iter::once(f.assertion_id.clone()).chain(f.others.iter().map(|o| o.assertion_id.clone())))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let claims = claims_behind(&snap, &ids)?;
        Ok(EntityView {
            entity,
            redirected_from,
            claims,
        })
    }

    pub fn entity_redirect(&self, caller: &Caller, vault: &VaultId, id: &ObjectId) -> Result<Redirect, ServerError> {
        self.read_access(caller, vault)?;
        let snap = self.engine.store().snapshot(vault)?;
        let (row, chain) = follow(&snap, id)?;
        Ok(Redirect {
            id: id.clone(),
            current_id: row.header.id,
            chain,
        })
    }

    /// From a mention to the entities it may mean: exact identity and alias matches first, then keyword matches on what's indexed. It never decides, so the caller sees every candidate.
    pub fn resolve_mention(
        &self,
        caller: &Caller,
        vault: &VaultId,
        mention: &str,
        type_name: Option<&str>,
        limit: usize,
    ) -> Result<EntityResolution, ServerError> {
        self.read_access(caller, vault)?;
        if mention.trim().is_empty() {
            return Err(ServerError::Invalid("a mention can't be empty".into()));
        }
        let folded = entity::fold_key(mention);
        let filter = LexicalFilter {
            kinds: vec![DocKind::Entity],
            types: type_name.map(String::from).into_iter().collect(),
            ..LexicalFilter::default()
        };
        // Read first, so anything retracted in the meantime is gone from the snapshot below.
        let mut degraded = vec![];
        let hits = match crate::lexical::build_query(mention, filter, limit) {
            Err(_) => vec![],
            Ok(query) => match self.lexical.search(vault, &query) {
                Ok(results) => results.hits,
                Err(e) => {
                    tracing::warn!(error = %e, "resolving a mention without the keyword index");
                    degraded.push("lexical".to_string());
                    vec![]
                }
            },
        };
        let snap = self.engine.store().snapshot(vault)?;
        let mut matched_on: Vec<(ObjectId, String)> = vec![];
        for m in entity::identity_matches(&snap, &folded, limit.max(1))? {
            if type_name.is_none_or(|t| t == m.type_name) && !matched_on.iter().any(|(id, _)| *id == m.entity_id) {
                matched_on.push((m.entity_id, m.field));
            }
        }
        let mut ids: Vec<ObjectId> = matched_on.iter().map(|(id, _)| id.clone()).collect();
        for h in &hits {
            if !ids.contains(&h.id) {
                ids.push(h.id.clone());
            }
        }
        ids.truncate(limit);
        let keys: Vec<RowKey> = ids.iter().map(current_key).collect();
        let rows = if keys.is_empty() { vec![] } else { snap.get_many(&keys)? };
        let mut candidates = vec![];
        for (id, row) in ids.iter().zip(rows) {
            let Some(Row::Entity(entity)) = row else { continue };
            if !entity.is_current() || type_name.is_some_and(|t| t != entity.type_name) {
                continue;
            }
            let exact = matched_on.iter().find(|(m, _)| m == id).map(|(_, f)| f.clone());
            let score = hits.iter().find(|h| h.id == *id).map_or(0.0, |h| h.score);
            candidates.push(EntityCandidate {
                entity,
                exact: exact.is_some(),
                matched_on: exact,
                score,
            });
        }
        Ok(EntityResolution { candidates, degraded })
    }

    /// Entities of a type sharing an identity value or alias. These are only suggestions: they're computed on read, and nothing is written until someone proposes a merge.
    pub fn same_as_suggestions(
        &self,
        caller: &Caller,
        vault: &VaultId,
        type_name: &str,
        after: Option<(&str, &str)>,
        limit: usize,
    ) -> Result<Vec<SharedValue>, ServerError> {
        self.read_access(caller, vault)?;
        let snap = self.engine.store().snapshot(vault)?;
        let types = entity::latest_types(&snap, &[type_name.to_string()])?;
        let t = types.get(type_name).ok_or(ServerError::NotFound)?;
        let mut fields: Vec<String> = entity::fields_of(&t.schema).into_iter().filter(|d| d.identity).map(|d| d.name).collect();
        fields.push(memex_types::ALIAS_FIELD.to_string());
        Ok(snap.shared_values(type_name, &fields, after, limit)?)
    }
}

fn current_key(id: &ObjectId) -> RowKey {
    RowKey {
        table: Table::Entity,
        id: id.clone(),
    }
}

/// The row an id leads to now, and the ids passed on the way there, the asked one first.
fn follow<S: Snapshot>(snap: &S, id: &ObjectId) -> Result<(EntityRow, Vec<ObjectId>), ServerError> {
    let mut at = id.clone();
    let mut chain = vec![];
    for _ in 0..64 {
        let Some(Row::Entity(row)) = snap.get(&current_key(&at))? else {
            return Err(ServerError::NotFound);
        };
        match row.redirect_to.clone() {
            Some(next) if row.header.status == Status::Superseded => {
                chain.push(at);
                at = next;
            }
            _ => return Ok((row, chain)),
        }
    }
    Err(ServerError::Unavailable(format!("entity {id} redirects too many times")))
}

/// These claims with their evidence and sources, in a fixed number of store calls.
fn claims_behind<S: Snapshot>(snap: &S, ids: &[ObjectId]) -> Result<HashMap<ObjectId, EntityClaim>, ServerError> {
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let keys: Vec<RowKey> = ids
        .iter()
        .map(|id| RowKey {
            table: Table::Assertion,
            id: id.clone(),
        })
        .collect();
    let rows = snap.get_many(&keys)?;
    let evidence = snap.evidence_of(ids)?;
    let segment_keys: Vec<RowKey> = evidence
        .iter()
        .map(|e| e.segment_id.clone())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .map(|id| RowKey { table: Table::Segment, id })
        .collect();
    let segments: HashMap<ObjectId, SegmentRow> = if segment_keys.is_empty() {
        HashMap::new()
    } else {
        snap.get_many(&segment_keys)?
            .into_iter()
            .filter_map(|r| match r {
                Some(Row::Segment(s)) => Some((s.header.id.clone(), s)),
                _ => None,
            })
            .collect()
    };
    let sources = crate::lexical::sources_by_id(snap, segments.values().map(|s| s.source_id.clone()))?;
    let mut out: HashMap<ObjectId, EntityClaim> = rows
        .into_iter()
        .filter_map(|r| match r {
            Some(Row::Assertion(assertion)) => Some((assertion.header.id.clone(), EntityClaim { assertion, evidence: vec![] })),
            _ => None,
        })
        .collect();
    for e in evidence {
        let segment = segments.get(&e.segment_id).cloned();
        let source = segment.as_ref().and_then(|s| sources.get(&s.source_id)).cloned();
        if let Some(claim) = out.get_mut(&e.assertion_id) {
            claim.evidence.push(ClaimEvidence { evidence: e, segment, source });
        }
    }
    Ok(out)
}
