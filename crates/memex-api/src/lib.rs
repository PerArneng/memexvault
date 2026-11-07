//! Pure: request and response types for the HTTP API, and their translation from domain data. No web framework.
mod entities;
mod extraction;
mod pages;
mod review;
pub use entities::*;
pub use extraction::*;
use memex_types::{
    AsOf, AssertionRow, ChangeSetRow, Citation, Conflict, DocKind, Event, EvidenceItem, EvidencePackage, EvidenceRow, FacetCount, Facets, Grant, JobRow,
    LexicalFilter, LexicalResults, ObjectId, Op, Principal, PrincipalKind, RenditionRow, Scope, SearchDebug, SearchMode, SearchRequest, SegmentRow,
    SignalScores, SourceRow, TokenRow, VaultId, VaultRow,
};
pub use pages::*;
pub use review::*;
use serde::{Deserialize, Serialize};
use utoipa::{IntoParams, ToSchema};

/// The one error format.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ErrorBody {
    pub error: ErrorDetail,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ErrorDetail {
    /// A stable machine-readable code, such as `not_found`.
    pub code: String,
    pub message: String,
    pub request_id: String,
    /// On `stale_version`: the row as it is now, so the caller can rebase.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schema(value_type = Option<Object>)]
    pub current: Option<serde_json::Value>,
}

/// Every list takes these. `sort` states the order, and `created` is the only one so far, oldest first.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ListParams {
    /// From `next_cursor` of the previous page.
    pub cursor: Option<String>,
    /// 1 to 200. The default is 50.
    pub limit: Option<u32>,
    /// `created`, the default.
    pub sort: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct ListRequest {
    pub cursor: Option<String>,
    pub limit: usize,
}

impl ListParams {
    /// Pure validation, so every list route behaves the same.
    pub fn validated(self) -> Result<ListRequest, String> {
        let limit = self.limit.unwrap_or(50);
        if !(1..=200).contains(&limit) {
            return Err(format!("limit must be from 1 to 200, not {limit}"));
        }
        if let Some(sort) = &self.sort
            && sort != "created"
        {
            return Err(format!("unknown sort '{sort}': the only sort here is 'created'"));
        }
        Ok(ListRequest {
            cursor: self.cursor,
            limit: limit as usize,
        })
    }
}

/// Splits a result fetched with one extra row into a page and its next cursor.
pub fn page_of<T>(mut items: Vec<T>, limit: usize, id: impl Fn(&T) -> String) -> (Vec<T>, Option<String>) {
    if items.len() > limit {
        items.truncate(limit);
        let next = items.last().map(&id);
        (items, next)
    } else {
        (items, None)
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct VaultDto {
    pub id: String,
    pub name: String,
    pub created_by: String,
    pub created_at_ms: i64,
}

impl From<VaultRow> for VaultDto {
    fn from(v: VaultRow) -> Self {
        Self {
            id: v.id.0,
            name: v.name,
            created_by: v.created_by,
            created_at_ms: v.created_at_ms,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct VaultPage {
    pub items: Vec<VaultDto>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct NameBody {
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PrincipalKindDto {
    Human,
    Agent,
}

impl From<PrincipalKind> for PrincipalKindDto {
    fn from(k: PrincipalKind) -> Self {
        match k {
            PrincipalKind::Human => Self::Human,
            PrincipalKind::Agent => Self::Agent,
        }
    }
}

impl From<PrincipalKindDto> for PrincipalKind {
    fn from(k: PrincipalKindDto) -> Self {
        match k {
            PrincipalKindDto::Human => Self::Human,
            PrincipalKindDto::Agent => Self::Agent,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct PrincipalDto {
    pub id: String,
    pub name: String,
    pub kind: PrincipalKindDto,
    pub server_admin: bool,
    pub disabled: bool,
    pub created_by: String,
    pub created_at_ms: i64,
}

impl From<Principal> for PrincipalDto {
    fn from(p: Principal) -> Self {
        Self {
            id: p.id,
            name: p.name,
            kind: p.kind.into(),
            server_admin: p.server_admin,
            disabled: p.disabled,
            created_by: p.created_by,
            created_at_ms: p.created_at_ms,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct PrincipalPage {
    pub items: Vec<PrincipalDto>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct NewPrincipalBody {
    pub name: String,
    pub kind: PrincipalKindDto,
    #[serde(default)]
    pub server_admin: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum ScopeDto {
    Read,
    Propose,
    Admin,
}

impl From<Scope> for ScopeDto {
    fn from(s: Scope) -> Self {
        match s {
            Scope::Read => Self::Read,
            Scope::Propose => Self::Propose,
            Scope::Admin => Self::Admin,
        }
    }
}

impl From<ScopeDto> for Scope {
    fn from(s: ScopeDto) -> Self {
        match s {
            ScopeDto::Read => Self::Read,
            ScopeDto::Propose => Self::Propose,
            ScopeDto::Admin => Self::Admin,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct GrantDto {
    pub vault_id: String,
    pub scope: ScopeDto,
}

impl From<Grant> for GrantDto {
    fn from(g: Grant) -> Self {
        Self {
            vault_id: g.vault.0,
            scope: g.scope.into(),
        }
    }
}

impl From<GrantDto> for Grant {
    fn from(g: GrantDto) -> Self {
        Self {
            vault: VaultId(g.vault_id),
            scope: g.scope.into(),
        }
    }
}

/// A token's details. The token itself and its hash never appear here.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct TokenDto {
    pub id: String,
    pub principal_id: String,
    pub name: String,
    pub grants: Vec<GrantDto>,
    pub created_by: String,
    pub created_at_ms: i64,
    pub expires_at_ms: Option<i64>,
    pub last_used_at_ms: Option<i64>,
    pub revoked_at_ms: Option<i64>,
}

impl From<TokenRow> for TokenDto {
    fn from(t: TokenRow) -> Self {
        Self {
            id: t.id,
            principal_id: t.principal_id,
            name: t.name,
            grants: t.grants.into_iter().map(Into::into).collect(),
            created_by: t.created_by,
            created_at_ms: t.created_at_ms,
            expires_at_ms: t.expires_at_ms,
            last_used_at_ms: t.last_used_at_ms,
            revoked_at_ms: t.revoked_at_ms,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct TokenPage {
    pub items: Vec<TokenDto>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct NewTokenBody {
    pub name: String,
    /// Whose token it is. The default is the caller.
    pub principal_id: Option<String>,
    #[serde(default)]
    pub grants: Vec<GrantDto>,
    pub expires_at_ms: Option<i64>,
}

/// Shown once, at creation.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CreatedTokenDto {
    pub token: String,
    pub details: TokenDto,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct WhoAmI {
    pub principal: PrincipalDto,
    pub token_id: String,
    pub grants: Vec<GrantDto>,
}

/// Signs in a browser. No `Debug`, so the token can't reach a log by accident.
#[derive(Deserialize, ToSchema)]
pub struct LoginBody {
    pub token: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct HealthDto {
    pub status: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct VersionDto {
    pub version: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct SourceDto {
    pub id: String,
    pub name: String,
    pub sha256: String,
    pub media_type: String,
    pub trust_tier: String,
    pub size: u64,
    pub logical_id: String,
    pub version_no: u32,
    /// What optimistic edits base themselves on.
    pub version: u32,
    #[schema(value_type = Object)]
    pub metadata: serde_json::Value,
    pub created_by: String,
    pub recorded_at_ms: i64,
}

impl From<SourceRow> for SourceDto {
    fn from(s: SourceRow) -> Self {
        Self {
            id: s.header.id.0,
            name: s.name,
            sha256: s.sha256,
            media_type: s.media_type,
            trust_tier: s.trust_tier,
            size: s.size,
            logical_id: s.logical_id.0,
            version_no: s.version_no,
            version: s.header.version,
            metadata: s.metadata,
            created_by: s.header.creator,
            recorded_at_ms: s.recorded_at_ms,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct SourcePage {
    pub items: Vec<SourceDto>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct SourceVersions {
    pub items: Vec<SourceDto>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct UploadQuery {
    /// The file's name, shown to people.
    pub name: String,
    /// `verified`, `trusted`, `unverified` or `untrusted`. The default is `unverified`.
    pub trust_tier: Option<String>,
    /// The Source this is a new version of.
    pub replaces: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct UploadResult {
    pub source: SourceDto,
    pub job_id: Option<String>,
    /// These bytes were already here, so nothing new was created.
    pub existing: bool,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct UploadLimitsDto {
    pub max_bytes: u64,
    pub allowed_media_types: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct JobDto {
    pub id: String,
    pub kind: String,
    /// `queued`, `running`, `done`, `dead` or `cancelled`.
    pub state: String,
    #[schema(value_type = Object)]
    pub input: serde_json::Value,
    pub max_attempts: u32,
    pub next_run_at_ms: i64,
    #[schema(value_type = Vec<Object>)]
    pub attempts: Vec<serde_json::Value>,
    pub created_at_ms: i64,
    pub finished_at_ms: Option<i64>,
    /// Set while the job waits for something that's away, such as the LLM.
    pub waiting_since_ms: Option<i64>,
}

impl From<JobRow> for JobDto {
    fn from(j: JobRow) -> Self {
        Self {
            id: j.header.id.0,
            kind: j.kind,
            state: json_text(&j.state),
            input: j.input,
            max_attempts: j.max_attempts,
            next_run_at_ms: j.next_run_at_ms,
            attempts: json_list(&j.attempts),
            created_at_ms: j.created_at_ms,
            finished_at_ms: j.finished_at_ms,
            waiting_since_ms: j.wait.map(|w| w.since_ms),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct JobPage {
    pub items: Vec<JobDto>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct JobFilter {
    pub kind: Option<String>,
    /// `queued`, `running`, `done`, `dead` or `cancelled`. `dead` is the dead-letter list.
    pub state: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct RenditionDto {
    pub id: String,
    pub source_id: String,
    pub parser: String,
    pub pipeline_version: u32,
    pub page_count: u32,
    pub segment_count: u32,
    pub created_at_ms: i64,
    pub superseded_by: Option<String>,
    /// How each page was read: `route`, mean OCR confidence in permille, and `flags`.
    #[schema(value_type = Vec<Object>)]
    pub pages: Vec<serde_json::Value>,
}

impl From<RenditionRow> for RenditionDto {
    fn from(r: RenditionRow) -> Self {
        Self {
            id: r.header.id.0,
            source_id: r.source_id.0,
            parser: r.parser,
            pipeline_version: r.pipeline_version,
            page_count: r.page_count,
            segment_count: r.segment_count,
            created_at_ms: r.created_at_ms,
            superseded_by: r.superseded_by.map(|i| i.0),
            pages: json_list(&r.pages),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct RenditionList {
    pub items: Vec<RenditionDto>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct RenditionDetail {
    pub rendition: RenditionDto,
    pub canonical_text: String,
    /// The word table and page facts. Absent for plain text.
    #[schema(value_type = Object)]
    pub word_table: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct SegmentDto {
    pub id: String,
    pub source_id: String,
    pub rendition_id: Option<String>,
    pub language: Option<String>,
    pub text: String,
    pub start: u64,
    pub end: u64,
    pub heading: Option<String>,
    pub page_from: Option<u32>,
    pub page_to: Option<u32>,
}

impl From<SegmentRow> for SegmentDto {
    fn from(s: SegmentRow) -> Self {
        Self {
            id: s.header.id.0,
            source_id: s.source_id.0,
            rendition_id: s.rendition_id.map(|i| i.0),
            language: s.language,
            text: s.text,
            start: s.start,
            end: s.end,
            heading: s.heading,
            page_from: s.page_from,
            page_to: s.page_to,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct SegmentPage {
    pub items: Vec<SegmentDto>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct FlaggedPageDto {
    pub source_id: String,
    pub rendition_id: String,
    pub page: u32,
    #[schema(value_type = Object)]
    pub outcome: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct FlaggedPagePage {
    pub items: Vec<FlaggedPageDto>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ReanchorFailureDto {
    pub evidence_id: String,
    pub assertion_id: String,
    pub segment_id: String,
    pub quote: String,
}

impl From<EvidenceRow> for ReanchorFailureDto {
    fn from(e: EvidenceRow) -> Self {
        Self {
            evidence_id: e.header.id.0,
            assertion_id: e.assertion_id.0,
            segment_id: e.segment_id.0,
            quote: e.quote,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ReanchorFailurePage {
    pub items: Vec<ReanchorFailureDto>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct RectDto {
    pub page: u32,
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct EvidenceLineageDto {
    pub evidence_id: String,
    pub quote: String,
    pub stance: String,
    pub segment_id: String,
    pub rendition_id: String,
    pub parser: String,
    pub source_id: String,
    pub source_name: String,
    pub source_sha256: String,
    pub trust_tier: String,
    /// A newer rendition exists. This still resolves.
    pub superseded: bool,
    /// The quote no longer resolves. Reported rather than guessed.
    pub broken: bool,
    /// Displayed coordinates in 1/100 pt, one per line.
    pub rects: Vec<RectDto>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct LineageDto {
    #[schema(value_type = Object)]
    pub assertion: serde_json::Value,
    pub changeset_id: String,
    /// Who proposed it, and whether a machine did.
    pub proposer: Option<String>,
    pub proposer_is_machine: Option<bool>,
    pub decided_by: Option<String>,
    pub rationale: Option<String>,
    pub evidence: Vec<EvidenceLineageDto>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct PageFactsDto {
    pub index: u32,
    pub width: i32,
    pub height: i32,
    pub rotation: i32,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct PageListDto {
    pub rendition_id: String,
    pub page_count: u32,
    pub allowed_dpi: Vec<u32>,
    pub pages: Vec<PageFactsDto>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ImageQuery {
    /// One of the allowed values. The default is 150.
    pub dpi: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct HighlightDto {
    pub source_id: String,
    pub page: u32,
    pub page_width: i32,
    pub page_height: i32,
    pub broken: bool,
    pub rects: Vec<RectDto>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ClaimRefDto {
    pub assertion_id: String,
    pub evidence_id: String,
    pub subject: String,
    pub predicate: String,
    #[schema(value_type = Object)]
    pub value: serde_json::Value,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ClaimRefPage {
    pub items: Vec<ClaimRefDto>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Deserialize, ToSchema)]
pub struct ProposalBody {
    /// The ops, as defined by `memex_types::Op`.
    #[schema(value_type = Vec<Object>)]
    pub ops: Vec<Op>,
    pub rationale: String,
    pub idempotency_key: String,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct ProposalQuery {
    /// Validate and decide, but write nothing.
    pub dry_run: Option<bool>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ChangeSetDto {
    pub id: String,
    /// `pending`, `committed` or `rejected`.
    pub state: String,
    pub proposer: String,
    pub proposer_is_machine: bool,
    pub rationale: String,
    pub idempotency_key: String,
    #[schema(value_type = Vec<Object>)]
    pub ops: Vec<serde_json::Value>,
    #[schema(value_type = Vec<Object>)]
    pub validators: Vec<serde_json::Value>,
    /// `auto_commit`, `review` or `reject`.
    pub risk: String,
    pub proposed_at_ms: i64,
    pub decided_by: Option<String>,
    pub decided_at_ms: Option<i64>,
    pub decision_reason: Option<String>,
    pub commit_seq: Option<u64>,
    pub reverts: Option<String>,
    pub reverted_by: Option<String>,
    /// What was proposed, when a person changed it before approving. `ops` then holds what was approved.
    #[schema(value_type = Option<Vec<Object>>)]
    pub original_ops: Option<Vec<serde_json::Value>>,
    /// The model behind an extraction, or otherwise whoever proposed it. Absent from before spec 025.
    pub extractor: Option<String>,
    pub prompt_version: Option<String>,
    pub item_types: Option<Vec<String>>,
    pub source_ids: Option<Vec<String>>,
}

fn json_text<T: Serialize>(v: &T) -> String {
    serde_json::to_value(v).ok().and_then(|j| j.as_str().map(str::to_string)).unwrap_or_default()
}

fn json_list<T: Serialize>(v: &[T]) -> Vec<serde_json::Value> {
    v.iter().filter_map(|x| serde_json::to_value(x).ok()).collect()
}

impl From<ChangeSetRow> for ChangeSetDto {
    fn from(c: ChangeSetRow) -> Self {
        Self {
            id: c.header.id.0,
            state: json_text(&c.state),
            proposer: c.proposer,
            proposer_is_machine: c.proposer_is_machine,
            rationale: c.rationale,
            idempotency_key: c.idempotency_key,
            ops: json_list(&c.ops),
            validators: json_list(&c.validators),
            risk: json_text(&c.risk),
            proposed_at_ms: c.proposed_at_ms,
            decided_by: c.decided_by,
            decided_at_ms: c.decided_at_ms,
            decision_reason: c.decision_reason,
            commit_seq: c.commit_seq,
            reverts: c.reverts.map(|i| i.0),
            reverted_by: c.reverted_by.map(|i| i.0),
            original_ops: c.original_ops.as_deref().map(json_list),
            extractor: c.origin.as_ref().map(|o| o.extractor.clone()),
            prompt_version: c.origin.as_ref().map(|o| o.prompt_version.clone()),
            item_types: c.origin.as_ref().map(|o| o.item_types.clone()),
            source_ids: c.origin.as_ref().map(|o| o.source_ids.iter().map(|i| i.0.clone()).collect()),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ChangeSetPage {
    pub items: Vec<ChangeSetDto>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ProposalResult {
    /// Absent on a dry run.
    pub changeset: Option<ChangeSetDto>,
    #[schema(value_type = Vec<Object>)]
    pub validators: Vec<serde_json::Value>,
    pub risk: String,
    pub contradictions: Vec<ContradictionDto>,
    pub replayed: bool,
    pub dry_run: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ContradictionDto {
    pub claim: String,
    pub other: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ContradictionPage {
    pub items: Vec<ContradictionDto>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ReasonBody {
    pub reason: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct InversePreview {
    #[schema(value_type = Vec<Object>)]
    pub mutations: Vec<serde_json::Value>,
    pub reversible: bool,
    pub blocked_because: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct EventDto {
    pub seq: u64,
    pub actor: String,
    pub time_ms: i64,
    pub kind: String,
    pub schema_version: u32,
    #[schema(value_type = Object)]
    pub payload: serde_json::Value,
}

impl From<Event> for EventDto {
    fn from(e: Event) -> Self {
        Self {
            seq: e.seq,
            actor: e.actor,
            time_ms: e.time_ms,
            kind: e.kind,
            schema_version: e.schema_version,
            payload: e.payload,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct EventPage {
    pub items: Vec<EventDto>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct AsOfQuery {
    pub subject: String,
    /// The sequence number to read as of.
    pub seq: u64,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct AssertionList {
    #[schema(value_type = Vec<Object>)]
    pub items: Vec<serde_json::Value>,
}

impl AssertionList {
    pub fn of(rows: &[AssertionRow]) -> Self {
        Self { items: json_list(rows) }
    }
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct StateFilter {
    /// `pending`, `committed` or `rejected`. `pending` is the review queue.
    pub state: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_defaults() {
        assert_eq!(ListParams::default().validated().unwrap(), ListRequest { cursor: None, limit: 50 });
    }

    #[test]
    fn list_bad_values_are_refused() {
        for limit in [0, 201] {
            assert!(
                ListParams {
                    limit: Some(limit),
                    ..Default::default()
                }
                .validated()
                .is_err()
            );
        }
        assert!(
            ListParams {
                sort: Some("name".into()),
                ..Default::default()
            }
            .validated()
            .is_err()
        );
        assert!(
            ListParams {
                limit: Some(200),
                sort: Some("created".into()),
                cursor: Some("x".into())
            }
            .validated()
            .is_ok()
        );
    }

    #[test]
    fn a_page_has_a_cursor_only_when_more_exist() {
        assert_eq!(page_of(vec![1, 2, 3], 2, |x: &i32| x.to_string()), (vec![1, 2], Some("2".into())));
        assert_eq!(page_of(vec![1, 2], 2, |x: &i32| x.to_string()), (vec![1, 2], None));
        assert_eq!(page_of(Vec::<i32>::new(), 2, |x: &i32| x.to_string()), (vec![], None));
    }
}

/// A keyword search. `q` holds words and double-quoted phrases, and everything else is a typed filter.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct LexicalSearchParams {
    pub q: String,
    /// 1 to 1000. The default is 20.
    pub limit: Option<u32>,
    /// `segment` or `claim`. All kinds by default.
    pub kind: Option<String>,
    pub source_id: Option<String>,
    pub epistemic: Option<String>,
    /// Keeps what's valid at this time, in milliseconds since 1970.
    pub valid_at_ms: Option<i64>,
}

impl LexicalSearchParams {
    /// Pure validation. The text itself is checked by the core's parser.
    pub fn validated(self) -> Result<(String, LexicalFilter, usize), String> {
        let limit = self.limit.unwrap_or(20);
        if !(1..=1000).contains(&limit) {
            return Err(format!("limit must be from 1 to 1000, not {limit}"));
        }
        let kinds = match self.kind.as_deref() {
            None => vec![],
            Some(k) => vec![DocKind::parse(k).ok_or_else(|| format!("kind must be segment, claim, entity, page or block, not {k}"))?],
        };
        let filter = LexicalFilter {
            kinds,
            source_ids: self.source_id.map(ObjectId).into_iter().collect(),
            epistemic: self.epistemic.into_iter().collect(),
            valid_at_ms: self.valid_at_ms,
            ..Default::default()
        };
        Ok((self.q, filter, limit as usize))
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct LexicalHitDto {
    pub id: String,
    /// `segment` or `claim`.
    pub kind: String,
    pub score: f32,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct LexicalResultsDto {
    pub hits: Vec<LexicalHitDto>,
    /// The last event this answer reflects, absent before the first.
    pub watermark: Option<u64>,
}

impl From<LexicalResults> for LexicalResultsDto {
    fn from(r: LexicalResults) -> Self {
        Self {
            hits: r
                .hits
                .into_iter()
                .map(|h| LexicalHitDto {
                    id: h.id.0,
                    kind: h.kind.as_str().into(),
                    score: h.score,
                })
                .collect(),
            watermark: r.watermark,
        }
    }
}

/// A search. `q` holds words and double-quoted phrases, and everything else is typed. List filters take comma-separated values.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct SearchParams {
    pub q: String,
    /// `hybrid`, the default, or `lexical` or `vector` for comparison and debugging.
    pub mode: Option<String>,
    /// Ranked items per page, 1 to 100. The default is 20.
    pub limit: Option<u32>,
    /// The most estimated tokens per page. The default is 4000.
    pub budget_tokens: Option<u32>,
    pub cursor: Option<String>,
    /// `segment` or `claim`.
    pub kind: Option<String>,
    /// A segment's media type, or a claim's predicate.
    #[serde(rename = "type")]
    #[param(rename = "type")]
    pub doc_type: Option<String>,
    pub source_id: Option<String>,
    pub trust_tier: Option<String>,
    pub epistemic: Option<String>,
    /// Keeps what's valid at this time, in milliseconds since 1970.
    pub valid_at_ms: Option<i64>,
    /// Answer as the vault stood just after this event. Slow: it reads the log.
    pub as_of: Option<u64>,
    /// As `as_of`, by time in milliseconds since 1970.
    pub as_of_ms: Option<i64>,
    /// Wait until the indexes have seen this event, for reading your own writes.
    pub min_seq: Option<u64>,
    /// How long `min_seq` may wait, 0 to 30000. The default is 2000.
    pub min_seq_wait_ms: Option<u32>,
    /// Also rerank the top 20 with a cross-encoder. Slower.
    pub rerank: Option<bool>,
    /// Add per-signal scores, the planner's path and timings.
    pub debug: Option<bool>,
    /// Add counts for the typed filters, over the candidates found.
    pub facets: Option<bool>,
    /// Whether to follow Edges from the best candidates: `auto`, the default, leaves it to the planner's rules, or `on` or `off`.
    pub expand: Option<String>,
    /// `<type>.<field>`: search within one entity field. Keyword only, since the vector index has no fields.
    pub attr: Option<String>,
}

fn csv(value: Option<String>) -> Vec<String> {
    value.map_or(vec![], |v| v.split(',').map(str::trim).filter(|s| !s.is_empty()).map(String::from).collect())
}

impl SearchParams {
    /// Pure validation. Returns the request and how long `min_seq` may wait, in milliseconds.
    pub fn validated(self) -> Result<(SearchRequest, u32), String> {
        let limit = self.limit.unwrap_or(20);
        if !(1..=100).contains(&limit) {
            return Err(format!("limit must be from 1 to 100, not {limit}"));
        }
        let budget = self.budget_tokens.unwrap_or(4000);
        if budget == 0 {
            return Err("budget_tokens must be at least 1".into());
        }
        let wait = self.min_seq_wait_ms.unwrap_or(2000);
        if wait > 30_000 {
            return Err(format!("min_seq_wait_ms must be at most 30000, not {wait}"));
        }
        let mode = match self.mode.as_deref() {
            None => SearchMode::Hybrid,
            Some(m) => SearchMode::parse(m).ok_or_else(|| format!("mode must be hybrid, lexical or vector, not {m}"))?,
        };
        let kinds = csv(self.kind)
            .iter()
            .map(|k| DocKind::parse(k).ok_or_else(|| format!("kind must be segment, claim, entity, page or block, not {k}")))
            .collect::<Result<_, _>>()?;
        let as_of = match (self.as_of, self.as_of_ms) {
            (Some(_), Some(_)) => return Err("give as_of or as_of_ms, not both".into()),
            (Some(seq), None) => Some(AsOf::Seq(seq)),
            (None, Some(ms)) => Some(AsOf::Time(ms)),
            (None, None) => None,
        };
        let expand = match self.expand.as_deref() {
            None => memex_types::Expand::Auto,
            Some(e) => memex_types::Expand::parse(e).ok_or_else(|| format!("expand must be auto, on or off, not {e}"))?,
        };
        let attr = match self.attr.as_deref() {
            None => None,
            Some(a) => {
                let (type_name, field) = a.split_once('.').ok_or_else(|| format!("attr must be <type>.<field>, not {a}"))?;
                Some(memex_types::AttrScope {
                    type_name: type_name.into(),
                    field: field.into(),
                })
            }
        };
        Ok((
            SearchRequest {
                text: self.q,
                mode,
                filter: LexicalFilter {
                    kinds,
                    source_ids: csv(self.source_id).into_iter().map(ObjectId).collect(),
                    epistemic: csv(self.epistemic),
                    valid_at_ms: self.valid_at_ms,
                    types: csv(self.doc_type),
                    trust_tiers: csv(self.trust_tier),
                    attr,
                },
                limit: limit as usize,
                budget_tokens: budget as usize,
                cursor: self.cursor,
                as_of,
                min_seq: self.min_seq,
                rerank: self.rerank.unwrap_or(false),
                debug: self.debug.unwrap_or(false),
                facets: self.facets.unwrap_or(false),
                expand,
            },
            wait,
        ))
    }
}

/// Where an item's text comes from. `anchor` is always there: `located`, `broken` or `none`.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct CitationDto {
    pub anchor: String,
    pub source_id: Option<String>,
    pub source_name: Option<String>,
    pub source_sha256: Option<String>,
    pub trust_tier: Option<String>,
    pub segment_id: Option<String>,
    pub evidence_id: Option<String>,
    pub rendition_id: Option<String>,
    pub start: Option<u64>,
    pub end: Option<u64>,
    pub page_from: Option<u32>,
    pub page_to: Option<u32>,
    pub rects: Vec<RectDto>,
    pub quote: Option<String>,
}

impl From<Citation> for CitationDto {
    fn from(c: Citation) -> Self {
        Self {
            anchor: c.status.as_str().into(),
            source_id: c.source_id.map(|i| i.0),
            source_name: c.source_name,
            source_sha256: c.source_sha256,
            trust_tier: c.trust_tier,
            segment_id: c.segment_id.map(|i| i.0),
            evidence_id: c.evidence_id.map(|i| i.0),
            rendition_id: c.rendition_id.map(|i| i.0),
            start: c.start,
            end: c.end,
            page_from: c.page_from,
            page_to: c.page_to,
            rects: c
                .rects
                .into_iter()
                .map(|(page, r)| RectDto {
                    page,
                    x0: r.x0,
                    y0: r.y0,
                    x1: r.x1,
                    y1: r.y1,
                })
                .collect(),
            quote: c.quote,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct ConflictDto {
    pub claim_id: String,
    pub text: String,
    pub epistemic: Option<String>,
    pub source_ids: Vec<String>,
}

impl From<Conflict> for ConflictDto {
    fn from(c: Conflict) -> Self {
        Self {
            claim_id: c.claim_id.0,
            text: c.text,
            epistemic: c.epistemic,
            source_ids: c.source_ids.into_iter().map(|i| i.0).collect(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct EvidenceItemDto {
    pub id: String,
    /// `claim`, `segment`, `entity`, `section`, `page` or `block`.
    pub kind: String,
    pub text: String,
    pub heading: Option<String>,
    pub language: Option<String>,
    pub epistemic: Option<String>,
    pub trust_tiers: Vec<String>,
    pub valid_from_ms: Option<i64>,
    pub valid_to_ms: Option<i64>,
    pub score: Option<f32>,
    pub estimated_tokens: usize,
    pub citations: Vec<CitationDto>,
    pub conflicts: Vec<ConflictDto>,
    /// The wiki page a page or block item belongs to.
    pub page_id: Option<String>,
}

impl From<EvidenceItem> for EvidenceItemDto {
    fn from(i: EvidenceItem) -> Self {
        Self {
            id: i.id.0,
            kind: i.kind.as_str().into(),
            text: i.text,
            heading: i.heading,
            language: i.language,
            epistemic: i.epistemic,
            trust_tiers: i.trust_tiers,
            valid_from_ms: i.valid_from_ms,
            valid_to_ms: i.valid_to_ms,
            score: i.score,
            estimated_tokens: i.estimated_tokens,
            citations: i.citations.into_iter().map(Into::into).collect(),
            conflicts: i.conflicts.into_iter().map(Into::into).collect(),
            page_id: i.page_id.map(|p| p.0),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct BudgetDto {
    pub limit: usize,
    pub used: usize,
    pub truncated: bool,
    pub smallest_item_needs: Option<usize>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct SignalScoresDto {
    pub id: String,
    pub lexical_rank: Option<usize>,
    pub lexical_score: Option<f32>,
    pub vector_rank: Option<usize>,
    pub vector_score: Option<f32>,
    pub fused: f32,
    pub rerank_score: Option<f32>,
}

impl From<SignalScores> for SignalScoresDto {
    fn from(s: SignalScores) -> Self {
        Self {
            id: s.id.0,
            lexical_rank: s.lexical_rank,
            lexical_score: s.lexical_score,
            vector_rank: s.vector_rank,
            vector_score: s.vector_score,
            fused: s.fused,
            rerank_score: s.rerank_score,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct StageTimingDto {
    pub stage: String,
    pub ms: f64,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct SearchDebugDto {
    pub lexical_weight: f32,
    pub vector_weight: f32,
    pub lexical_terms: usize,
    pub dropped_terms: usize,
    pub best_vector_score: Option<f32>,
    pub term_coverage: Option<f32>,
    pub candidates_before_fence: usize,
    pub candidates_after_fence: usize,
    /// Candidates that following Edges added.
    pub expanded: usize,
    pub signals: Vec<SignalScoresDto>,
    pub timings: Vec<StageTimingDto>,
}

impl From<SearchDebug> for SearchDebugDto {
    fn from(d: SearchDebug) -> Self {
        Self {
            lexical_weight: d.lexical_weight,
            vector_weight: d.vector_weight,
            lexical_terms: d.lexical_terms,
            dropped_terms: d.dropped_terms,
            best_vector_score: d.best_vector_score,
            term_coverage: d.term_coverage,
            candidates_before_fence: d.candidates_before_fence,
            candidates_after_fence: d.candidates_after_fence,
            expanded: d.expanded,
            signals: d.signals.into_iter().map(Into::into).collect(),
            timings: d.timings_ms.into_iter().map(|(stage, ms)| StageTimingDto { stage, ms }).collect(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct FacetCountDto {
    pub value: String,
    pub count: u64,
}

fn facet_counts(counts: Vec<FacetCount>) -> Vec<FacetCountDto> {
    counts
        .into_iter()
        .map(|c| FacetCountDto {
            value: c.value,
            count: c.count,
        })
        .collect()
}

/// Counts over the candidates found, not the whole vault.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct FacetsDto {
    pub kind: Vec<FacetCountDto>,
    #[serde(rename = "type")]
    pub doc_type: Vec<FacetCountDto>,
    pub source_id: Vec<FacetCountDto>,
    pub trust_tier: Vec<FacetCountDto>,
    pub epistemic: Vec<FacetCountDto>,
}

impl From<Facets> for FacetsDto {
    fn from(f: Facets) -> Self {
        Self {
            kind: facet_counts(f.kind),
            doc_type: facet_counts(f.doc_type),
            source_id: facet_counts(f.source_id),
            trust_tier: facet_counts(f.trust_tier),
            epistemic: facet_counts(f.epistemic),
        }
    }
}

/// The answer to a search: claims first, then segments, then context sections, within a token budget.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct EvidencePackageDto {
    /// `exact`, `identifier`, `natural`, `filtered` or `as_of`.
    pub path: String,
    pub items: Vec<EvidenceItemDto>,
    /// Nothing here looks like an answer.
    pub low_relevance: bool,
    pub notices: Vec<String>,
    pub budget: BudgetDto,
    pub next_cursor: Option<String>,
    pub watermark: Option<u64>,
    pub as_of: Option<u64>,
    /// Signals that couldn't answer, so the result rests on the others.
    pub degraded: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debug: Option<SearchDebugDto>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facets: Option<FacetsDto>,
}

impl From<EvidencePackage> for EvidencePackageDto {
    fn from(p: EvidencePackage) -> Self {
        Self {
            path: p.path.as_str().into(),
            items: p.items.into_iter().map(Into::into).collect(),
            low_relevance: p.low_relevance,
            notices: p.notices,
            budget: BudgetDto {
                limit: p.budget.limit,
                used: p.budget.used,
                truncated: p.budget.truncated,
                smallest_item_needs: p.budget.smallest_item_needs,
            },
            next_cursor: p.next_cursor,
            watermark: p.watermark,
            as_of: p.as_of,
            degraded: p.degraded,
            debug: p.debug.map(Into::into),
            facets: p.facets.map(Into::into),
        }
    }
}
