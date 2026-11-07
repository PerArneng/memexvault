//! Wiki pages: their blocks, their slugs and their history. A page's Markdown is its edit format, and its blocks are that Markdown split up, so either gives the other.
use crate::ObjectId;
use crate::rows::Header;
use serde::{Deserialize, Serialize};

/// A wiki link's edge. It runs from a page to `slug:<slug>`, the target as written, so it needs no change when the target appears, moves or goes.
pub const WIKI_LINK: &str = "wiki_link";
/// A claim reference's edge. It runs from a page to the claim's id.
pub const CLAIM_REF: &str = "claim_ref";
const SLUG_PREFIX: &str = "slug:";

/// What a wiki link's edge points at.
pub fn slug_target(slug: &str) -> ObjectId {
    ObjectId(format!("{SLUG_PREFIX}{slug}"))
}

/// The slug a wiki link's edge points at, if it does point at one.
pub fn slug_of_target(id: &ObjectId) -> Option<&str> {
    id.0.strip_prefix(SLUG_PREFIX)
}

/// The id of a page's revision. Zero padding makes id order the revision order.
pub fn revision_id(page: &ObjectId, revision: u32) -> ObjectId {
    ObjectId(format!("{}@{revision:08}", page.0))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlockKind {
    Heading,
    Paragraph,
    List,
    Quote,
    Code,
    Table,
    Rule,
}

/// One block of a page as it is now. Its id survives edits that keep or change its text, so links and evidence can name it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockRow {
    pub header: Header,
    pub page_id: ObjectId,
    pub position: u32,
    pub kind: BlockKind,
    /// Verbatim Markdown.
    pub text: String,
    /// The nearest heading above it.
    pub heading: Option<String>,
}

/// A slug held by a page: its current one, or an old one that redirects. A slug belongs to one page at a time, which the primary key enforces.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlugRow {
    /// `header.id` is the slug.
    pub header: Header,
    pub page_id: ObjectId,
}

/// A block as a revision holds it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevisionBlock {
    pub id: ObjectId,
    pub kind: BlockKind,
    pub text: String,
}

/// A page as it stood at one version. Revisions are never edited, and outlive the page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevisionRow {
    pub header: Header,
    pub page_id: ObjectId,
    pub revision: u32,
    pub title: String,
    pub slug: String,
    /// Absent where a list of revisions leaves them out to stay small.
    #[serde(default)]
    pub blocks: Vec<RevisionBlock>,
    pub changeset_id: ObjectId,
    pub created_at_ms: i64,
    /// The revision this one restored, on a revert.
    pub restored_from: Option<u32>,
}

/// Which way an edge runs from the ids asked about.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EdgeDirection {
    From,
    To,
    Both,
}
