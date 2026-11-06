//! Request and response types for entity types and entities.
use crate::{ListParams, ListRequest, ProposalResult};
use memex_types::{ClaimedValue, EntityField, EntityRow, EntityTypeRow, EvidenceInput, EvidenceRow, ObjectId, SegmentRow, SharedValue, SourceRow, Stance};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::{IntoParams, ToSchema};

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct EntityTypeDto {
    pub name: String,
    pub version: u32,
    /// The version's JSON Schema, with the annotations `searchable`, `filterable` and `identity` on its fields.
    #[schema(value_type = Object)]
    pub schema: Value,
    /// `seed`, `new`, `additive` or `breaking`.
    pub change: String,
    pub reasons: Vec<String>,
    /// The ChangeSet that made it, absent on a seed type's built-in first version.
    pub changeset_id: Option<String>,
    pub created_at_ms: i64,
}

impl From<EntityTypeRow> for EntityTypeDto {
    fn from(t: EntityTypeRow) -> Self {
        Self {
            name: t.name,
            version: t.version_no,
            schema: t.schema,
            change: t.change,
            reasons: t.reasons,
            changeset_id: t.changeset_id.map(|c| c.0),
            created_at_ms: t.created_at_ms,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct EntityTypePage {
    pub items: Vec<EntityTypeDto>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct EntityTypeVersions {
    /// Oldest first.
    pub items: Vec<EntityTypeDto>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct NewEntityTypeBody {
    pub name: String,
    /// The version this builds on, as the type list gave it. Leave it out for a new type.
    pub base_version: Option<u32>,
    #[schema(value_type = Object)]
    pub schema: Value,
    pub rationale: String,
    pub idempotency_key: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct TypePreviewBody {
    #[schema(value_type = Object)]
    pub schema: Value,
}

/// What submitting this schema would do for this caller. Nothing is written.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct TypePreviewDto {
    /// Why a submission would be refused. Absent if it's acceptable.
    pub problem: Option<String>,
    /// `new`, `additive` or `breaking`.
    pub change: String,
    pub reasons: Vec<String>,
    pub current_version: Option<u32>,
    pub next_version: u32,
    /// `auto_commit`, `review` or `reject`.
    pub decision: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct EvidenceInputDto {
    pub segment_id: String,
    /// Verbatim from the segment, apart from runs of whitespace.
    pub quote: String,
    /// `supports`, the default, or `refutes`.
    pub stance: Option<String>,
}

impl EvidenceInputDto {
    fn into_input(self) -> Result<EvidenceInput, String> {
        let stance = match self.stance.as_deref() {
            None | Some("supports") => Stance::Supports,
            Some("refutes") => Stance::Refutes,
            Some(other) => return Err(format!("stance must be supports or refutes, not {other}")),
        };
        Ok(EvidenceInput {
            segment_id: ObjectId(self.segment_id),
            quote: self.quote,
            stance,
        })
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ReplacesDto {
    pub assertion_id: String,
    /// The claim's version when it was read.
    pub version: u32,
}

/// One proposed value. It's added beside any others on the field, unless it says which claim it replaces.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct FieldInputDto {
    pub field: String,
    #[schema(value_type = Object)]
    pub value: Value,
    #[serde(default)]
    pub evidence: Vec<EvidenceInputDto>,
    pub replaces: Option<ReplacesDto>,
}

/// A value in plain types, for a crate that can't see core's.
pub struct FieldParts {
    pub field: String,
    pub value: Value,
    pub evidence: Vec<EvidenceInput>,
    pub replaces: Option<(ObjectId, u32)>,
}

impl FieldInputDto {
    pub fn parts(self) -> Result<FieldParts, String> {
        Ok(FieldParts {
            field: self.field,
            value: self.value,
            evidence: self.evidence.into_iter().map(EvidenceInputDto::into_input).collect::<Result<_, _>>()?,
            replaces: self.replaces.map(|r| (ObjectId(r.assertion_id), r.version)),
        })
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct NewEntityBody {
    #[serde(rename = "type")]
    pub type_name: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    pub fields: Vec<FieldInputDto>,
    pub rationale: String,
    pub idempotency_key: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct EntityEditBody {
    pub base_version: u32,
    /// Replaces the aliases. Leave it out to keep them.
    pub aliases: Option<Vec<String>>,
    #[serde(default)]
    pub fields: Vec<FieldInputDto>,
    pub rationale: String,
    pub idempotency_key: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct MergeBody {
    /// The entity that stays.
    pub into: String,
    pub rationale: String,
    pub idempotency_key: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct SplitBody {
    pub base_version: u32,
    /// The active claims that move to the new entity.
    #[serde(default)]
    pub moved: Vec<String>,
    /// The aliases that move to the new entity.
    #[serde(default)]
    pub aliases: Vec<String>,
    /// Anything else the new entity needs to be valid, such as its own name.
    #[serde(default)]
    pub fields: Vec<FieldInputDto>,
    pub rationale: String,
    pub idempotency_key: String,
}

/// A proposal about an entity, and the entity it's about.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct EntityProposalResult {
    pub entity_id: String,
    #[serde(flatten)]
    pub result: ProposalResult,
}

/// An entity's current state, as lists show it.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct EntityDto {
    pub id: String,
    #[serde(rename = "type")]
    pub type_name: String,
    pub type_version: u32,
    pub version: u32,
    pub aliases: Vec<String>,
    /// Each field's current value.
    #[schema(value_type = Object)]
    pub attributes: Value,
    /// The fields with a claim that disagrees with the current value.
    pub conflicted_fields: Vec<String>,
    pub merged_from: Vec<String>,
    pub updated_at_ms: i64,
}

fn conflicted(f: &EntityField) -> bool {
    f.others.iter().any(|o| o.value != f.value)
}

impl From<EntityRow> for EntityDto {
    fn from(e: EntityRow) -> Self {
        Self {
            id: e.header.id.0,
            type_name: e.type_name,
            type_version: e.type_version,
            version: e.header.version,
            aliases: e.aliases,
            attributes: Value::Object(e.fields.iter().map(|f| (f.field.clone(), f.value.clone())).collect()),
            conflicted_fields: e.fields.iter().filter(|f| conflicted(f)).map(|f| f.field.clone()).collect(),
            merged_from: e.merged_from.into_iter().map(|m| m.0).collect(),
            updated_at_ms: e.updated_at_ms,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct EntityPage {
    pub items: Vec<EntityDto>,
    pub next_cursor: Option<String>,
}

/// One piece of evidence, and the way to its source. `assertions/{assertion_id}/lineage` and `.../evidence/{evidence_id}/highlight` give the full region.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct FieldEvidenceDto {
    pub evidence_id: String,
    pub quote: String,
    /// `supports` or `refutes`.
    pub stance: String,
    pub segment_id: String,
    pub source_id: Option<String>,
    pub source_name: Option<String>,
    pub trust_tier: Option<String>,
    pub rendition_id: Option<String>,
    pub page_from: Option<u32>,
    pub page_to: Option<u32>,
    /// Re-anchoring on a new rendition failed.
    pub anchor_broken: bool,
}

impl FieldEvidenceDto {
    pub fn new(e: EvidenceRow, segment: Option<SegmentRow>, source: Option<SourceRow>) -> Self {
        Self {
            evidence_id: e.header.id.0,
            quote: e.quote,
            stance: crate::json_text(&e.stance),
            segment_id: e.segment_id.0,
            source_id: source.as_ref().map(|s| s.header.id.0.clone()),
            source_name: source.as_ref().map(|s| s.name.clone()),
            trust_tier: source.map(|s| s.trust_tier),
            rendition_id: segment.as_ref().and_then(|s| s.rendition_id.clone().map(|r| r.0)),
            page_from: segment.as_ref().and_then(|s| s.page_from),
            page_to: segment.as_ref().and_then(|s| s.page_to),
            anchor_broken: e.anchor_broken,
        }
    }
}

/// A claim on a field. Every value traces to one, and a person's claim may have no evidence.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct FieldClaimDto {
    pub assertion_id: String,
    #[schema(value_type = Object)]
    pub value: Value,
    /// `verified`, `asserted`, `inferred` or `disputed`.
    pub epistemic: Option<String>,
    /// A machine made it.
    pub machine: Option<bool>,
    pub creator: Option<String>,
    pub changeset_id: Option<String>,
    pub evidence: Vec<FieldEvidenceDto>,
}

/// A field: its current value, why it's the one, and every other claim. `conflicts` lists those that disagree, and `agreeing` those that don't.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct EntityFieldDto {
    pub field: String,
    #[schema(value_type = Object)]
    pub value: Value,
    pub why: String,
    pub current: FieldClaimDto,
    pub conflicts: Vec<FieldClaimDto>,
    pub agreeing: Vec<FieldClaimDto>,
}

/// The rows behind one claim, as a route has read them.
pub struct ClaimParts {
    pub epistemic: String,
    pub machine: bool,
    pub creator: String,
    pub changeset_id: String,
    pub evidence: Vec<FieldEvidenceDto>,
}

impl EntityFieldDto {
    pub fn new(f: EntityField, mut lookup: impl FnMut(&ObjectId) -> Option<ClaimParts>) -> Self {
        let mut claim = |id: &ObjectId, value: Value| -> FieldClaimDto {
            let parts = lookup(id);
            let (epistemic, machine, creator, changeset_id, evidence) = match parts {
                Some(p) => (Some(p.epistemic), Some(p.machine), Some(p.creator), Some(p.changeset_id), p.evidence),
                None => (None, None, None, None, vec![]),
            };
            FieldClaimDto {
                assertion_id: id.0.clone(),
                value,
                epistemic,
                machine,
                creator,
                changeset_id,
                evidence,
            }
        };
        let current = claim(&f.assertion_id, f.value.clone());
        let (mut conflicts, mut agreeing) = (vec![], vec![]);
        for ClaimedValue { assertion_id, value } in f.others {
            let same = value == f.value;
            let c = claim(&assertion_id, value);
            if same {
                agreeing.push(c);
            } else {
                conflicts.push(c);
            }
        }
        Self {
            field: f.field,
            value: f.value,
            why: f.why,
            current,
            conflicts,
            agreeing,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct EntityDetailDto {
    pub id: String,
    #[serde(rename = "type")]
    pub type_name: String,
    pub type_version: u32,
    pub version: u32,
    pub aliases: Vec<String>,
    pub merged_from: Vec<String>,
    /// The id asked for, when it has been merged into this one.
    pub redirected_from: Option<String>,
    pub changeset_id: String,
    pub updated_at_ms: i64,
    pub fields: Vec<EntityFieldDto>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct RedirectDto {
    pub id: String,
    /// The entity this id leads to now, which is itself unless it has been merged.
    pub current_id: String,
    /// The ids passed on the way there, the asked one first.
    pub chain: Vec<String>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ResolveParams {
    pub mention: String,
    /// Only entities of this type.
    #[serde(rename = "type")]
    #[param(rename = "type")]
    pub type_name: Option<String>,
    /// 1 to 50. The default is 10.
    pub limit: Option<u32>,
}

impl ResolveParams {
    pub fn limit(&self) -> Result<usize, String> {
        match self.limit.unwrap_or(10) {
            n @ 1..=50 => Ok(n as usize),
            n => Err(format!("limit must be from 1 to 50, not {n}")),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CandidateDto {
    pub entity: EntityDto,
    /// The identity field or `$alias` the mention matches exactly. Absent on a keyword match.
    pub matched_on: Option<String>,
    pub exact: bool,
    /// The keyword score, comparable only within this answer.
    pub score: f32,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ResolveResult {
    /// Exact matches first. Choosing among them is the caller's decision.
    pub candidates: Vec<CandidateDto>,
    /// `lexical`, if only exact matches could be looked for.
    pub degraded: Vec<String>,
}

/// One entity list request: `type`, any number of `filter=<field>:<op>:<value>`, and the usual cursor and limit.
#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct EntityListParams {
    #[serde(rename = "type")]
    #[param(rename = "type")]
    pub type_name: String,
    /// `<field>:<op>:<value>`, where op is `eq`, `lt`, `lte`, `gt` or `gte`. Repeat it to require several. The field must be filterable.
    pub filter: Option<String>,
    pub cursor: Option<String>,
    pub limit: Option<u32>,
    pub sort: Option<String>,
}

/// What an entity list request comes to, once checked.
#[derive(Debug, PartialEq, Eq)]
pub struct EntityListRequest {
    pub type_name: String,
    pub filters: Vec<(String, String, String)>,
    pub list: ListRequest,
}

/// Parses the query pairs. A repeated parameter needs the pairs themselves, which a struct can't hold.
pub fn entity_list_request(pairs: Vec<(String, String)>) -> Result<EntityListRequest, String> {
    let mut type_name = None;
    let mut filters = vec![];
    let mut params = ListParams {
        cursor: None,
        limit: None,
        sort: None,
    };
    for (k, v) in pairs {
        match k.as_str() {
            "type" => type_name = Some(v),
            "cursor" => params.cursor = Some(v),
            "sort" => params.sort = Some(v),
            "limit" => params.limit = Some(v.parse().map_err(|_| format!("limit must be a number, not {v}"))?),
            "filter" => {
                let mut parts = v.splitn(3, ':');
                let (Some(field), Some(op), Some(value)) = (parts.next(), parts.next(), parts.next()) else {
                    return Err(format!("a filter is <field>:<op>:<value>, not {v}"));
                };
                filters.push((field.to_string(), op.to_string(), value.to_string()));
            }
            _ => return Err(format!("unknown parameter {k}")),
        }
    }
    Ok(EntityListRequest {
        type_name: type_name.ok_or("type is required")?,
        filters,
        list: params.validated()?,
    })
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct SameAsParams {
    #[serde(rename = "type")]
    #[param(rename = "type")]
    pub type_name: String,
    pub cursor: Option<String>,
    /// 1 to 200. The default is 50.
    pub limit: Option<u32>,
    /// By field and value, the only order.
    pub sort: Option<String>,
}

/// A suggestion, never a decision: these entities share an identity value or alias. Merging them is a proposal, which a person approves.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct SameAsDto {
    pub field: String,
    /// The shared value, folded.
    pub value: String,
    pub entity_ids: Vec<String>,
}

impl From<SharedValue> for SameAsDto {
    fn from(s: SharedValue) -> Self {
        Self {
            field: s.field,
            value: s.text,
            entity_ids: s.entity_ids.into_iter().map(|i| i.0).collect(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct SameAsPage {
    pub items: Vec<SameAsDto>,
    pub next_cursor: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(items: &[(&str, &str)]) -> Vec<(String, String)> {
        items.iter().map(|(a, b)| ((*a).to_string(), (*b).to_string())).collect()
    }

    #[test]
    fn an_entity_list_request_reads_repeated_filters() {
        let r = entity_list_request(pairs(&[
            ("type", "Product"),
            ("filter", "price:lt:100"),
            ("filter", "released:gte:2024-01-01T00:00:00Z"),
            ("limit", "5"),
        ]))
        .unwrap();
        assert_eq!(r.type_name, "Product");
        assert_eq!(r.filters[0], ("price".to_string(), "lt".to_string(), "100".to_string()));
        assert_eq!(r.filters[1].2, "2024-01-01T00:00:00Z", "a value can hold colons");
        assert_eq!(r.list.limit, 5);
    }

    #[test]
    fn a_bad_entity_list_request_says_what_is_wrong() {
        for bad in [
            pairs(&[]),
            pairs(&[("type", "T"), ("filter", "price<100")]),
            pairs(&[("type", "T"), ("limit", "many")]),
            pairs(&[("type", "T"), ("limit", "0")]),
            pairs(&[("type", "T"), ("nope", "1")]),
        ] {
            assert!(entity_list_request(bad).is_err());
        }
    }
}
