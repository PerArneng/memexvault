//! Request and response types for wiki pages.
use crate::{FieldEvidenceDto, ProposalResult, json_text};
use memex_types::{PageRow, RevisionBlock, RevisionRow};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::{IntoParams, ToSchema};

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct PageSummaryDto {
    pub id: String,
    pub title: String,
    pub slug: String,
    /// Old slugs, oldest first, that still lead here.
    pub redirects: Vec<String>,
    /// The version an edit bases itself on, which is also the newest revision.
    pub version: u32,
    pub creator: String,
    pub updated_at_ms: i64,
}

impl From<PageRow> for PageSummaryDto {
    fn from(p: PageRow) -> Self {
        Self {
            id: p.header.id.0,
            title: p.title,
            slug: p.slug,
            redirects: p.redirects,
            version: p.header.version,
            creator: p.header.creator,
            updated_at_ms: p.updated_at_ms,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct PageSummaryPage {
    pub items: Vec<PageSummaryDto>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct NewPageBody {
    pub title: String,
    /// Left out, it's made from the title.
    pub slug: Option<String>,
    /// Markdown, with `[[wiki links]]` and `{{claim:<id>}}` references.
    pub body: String,
    pub rationale: String,
    pub idempotency_key: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct PageEditBody {
    /// The version this edit was made from. If the page has moved on, the answer is a conflict with its current state.
    pub base_version: u32,
    pub body: String,
    pub rationale: String,
    pub idempotency_key: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct PageRenameBody {
    pub base_version: u32,
    pub title: String,
    /// Left out, it's made from the title. The old slug keeps leading here.
    pub slug: Option<String>,
    pub rationale: String,
    pub idempotency_key: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct PageRevertBody {
    pub base_version: u32,
    /// The revision to make current again, as a new revision.
    pub revision: u32,
    pub rationale: String,
    pub idempotency_key: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct PageDeleteBody {
    pub base_version: u32,
    pub rationale: String,
    pub idempotency_key: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct PageImportBody {
    /// A page as export gives it: front matter with a JSON-quoted `title`, and optionally `slug` and `redirects`, then the Markdown. Without front matter, its first heading is the title.
    pub markdown: String,
    /// Needed to edit the page already holding its slug.
    pub base_version: Option<u32>,
    pub rationale: String,
    pub idempotency_key: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct PageProposalResult {
    pub page_id: String,
    pub result: ProposalResult,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct PageImportResult {
    pub page_id: String,
    /// False when it edited an existing page.
    pub created: bool,
    pub result: ProposalResult,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct BlockDto {
    pub id: String,
    /// `heading`, `paragraph`, `list`, `quote`, `code`, `table` or `rule`.
    pub kind: String,
    /// Verbatim Markdown.
    pub text: String,
}

impl From<RevisionBlock> for BlockDto {
    fn from(b: RevisionBlock) -> Self {
        Self {
            id: b.id.0,
            kind: json_text(&b.kind),
            text: b.text,
        }
    }
}

/// What a page's `{{claim:<id>}}` shows now, whichever revision of the page is being read.
#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct PageClaimDto {
    /// The id the page names.
    pub claim_id: String,
    /// `current`, `superseded` or `missing`.
    pub status: String,
    /// The claim that stands now: the one named, or what replaced it. Absent when missing.
    pub current_claim_id: Option<String>,
    pub subject: Option<String>,
    pub predicate: Option<String>,
    #[schema(value_type = Option<Object>)]
    pub value: Option<Value>,
    /// `verified`, `asserted`, `inferred` or `disputed`.
    pub epistemic: Option<String>,
    pub citations: Vec<FieldEvidenceDto>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct PageDetailDto {
    pub id: String,
    /// The version an edit bases itself on.
    pub current_revision: u32,
    pub current_slug: String,
    pub redirects: Vec<String>,
    /// The revision shown.
    pub revision: u32,
    pub title: String,
    /// The slug at the revision shown.
    pub slug: String,
    /// Canonical Markdown: exactly its blocks, joined.
    pub body: String,
    pub blocks: Vec<BlockDto>,
    pub claims: Vec<PageClaimDto>,
    pub creator: String,
    pub updated_at_ms: i64,
    /// The revision this one restored, on a revert.
    pub restored_from: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct RevisionDto {
    pub revision: u32,
    pub title: String,
    pub slug: String,
    pub creator: String,
    pub changeset_id: String,
    pub created_at_ms: i64,
    pub restored_from: Option<u32>,
}

impl From<RevisionRow> for RevisionDto {
    fn from(r: RevisionRow) -> Self {
        Self {
            revision: r.revision,
            title: r.title,
            slug: r.slug,
            creator: r.header.creator,
            changeset_id: r.changeset_id.0,
            created_at_ms: r.created_at_ms,
            restored_from: r.restored_from,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct RevisionPage {
    pub items: Vec<RevisionDto>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct PageReadParams {
    /// A past revision. The current one by default.
    pub revision: Option<u32>,
}

#[derive(Debug, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct PageDiffParams {
    pub from: u32,
    pub to: u32,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct BlockChangeDto {
    pub block_id: String,
    /// `added`, `removed`, `changed` or `moved`.
    pub change: String,
    pub before: Option<String>,
    pub after: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct PageDiffDto {
    pub from: u32,
    pub to: u32,
    /// Old and new, if it changed.
    pub title: Option<Vec<String>>,
    pub slug: Option<Vec<String>>,
    pub blocks: Vec<BlockChangeDto>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct BacklinkDto {
    pub edge_id: String,
    pub from_page: String,
    pub from_title: Option<String>,
    pub from_slug: Option<String>,
    /// The block holding the link, which is its evidence.
    pub block_id: Option<String>,
    /// The slug as the link wrote it, which may be an old one.
    pub via_slug: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct BacklinkPage {
    pub items: Vec<BacklinkDto>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct OutgoingLinkDto {
    pub edge_id: String,
    /// `wiki_link` or `claim_ref`.
    pub kind: String,
    pub block_id: Option<String>,
    /// The slug or claim id as written.
    pub target: String,
    pub to_page: Option<String>,
    /// Whether the target exists.
    pub resolved: bool,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct OutgoingLinkPage {
    pub items: Vec<OutgoingLinkDto>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct UnresolvedLinkDto {
    pub edge_id: String,
    /// `wiki_link` or `claim_ref`.
    pub kind: String,
    pub from_page: String,
    pub block_id: Option<String>,
    /// The slug or claim id that leads nowhere.
    pub target: String,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct UnresolvedLinkPage {
    pub items: Vec<UnresolvedLinkDto>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct PageRedirectDto {
    pub slug: String,
    pub page_id: String,
    pub current_slug: String,
    /// The slug asked for is an old one.
    pub redirected: bool,
}

/// Which links a neighborhood shows.
#[derive(Debug, Default, Deserialize, IntoParams)]
#[into_params(parameter_in = Query)]
pub struct NeighborhoodParams {
    /// Comma-separated edge kinds. The default is `wiki_link,claim_ref,contradicts`.
    pub kinds: Option<String>,
}

impl NeighborhoodParams {
    /// The kinds asked for, or the default ones.
    pub fn kinds(&self) -> Vec<String> {
        let asked: Vec<String> = self
            .kinds
            .iter()
            .flat_map(|k| k.split(','))
            .map(str::trim)
            .filter(|k| !k.is_empty())
            .map(String::from)
            .collect();
        if asked.is_empty() {
            DEFAULT_NEIGHBOR_KINDS.iter().map(|k| k.to_string()).collect()
        } else {
            asked
        }
    }
}

const DEFAULT_NEIGHBOR_KINDS: [&str; 3] = ["wiki_link", "claim_ref", "contradicts"];

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct NeighborDto {
    pub edge_id: String,
    pub kind: String,
    /// `out` when the object asked about is where the link starts, or `in`.
    pub direction: String,
    pub from: String,
    pub to: String,
    pub block_id: Option<String>,
    /// What's at the other end. Null is a link to a slug no page holds.
    pub neighbor: Option<String>,
    /// The other end's title, when it's a page.
    pub title: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, ToSchema)]
pub struct NeighborPage {
    pub items: Vec<NeighborDto>,
    pub next_cursor: Option<String>,
}
