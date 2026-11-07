//! What a keyword index stores and answers. No engine type or query syntax appears here.
use crate::ObjectId;
use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocKind {
    Segment,
    Claim,
    Entity,
    Page,
    Block,
}

impl DocKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Segment => "segment",
            Self::Claim => "claim",
            Self::Entity => "entity",
            Self::Page => "page",
            Self::Block => "block",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "segment" => Some(Self::Segment),
            "claim" => Some(Self::Claim),
            "entity" => Some(Self::Entity),
            "page" => Some(Self::Page),
            "block" => Some(Self::Block),
            _ => None,
        }
    }
}

/// One field of an entity, searchable on its own as well as through the document's text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttrValue {
    pub type_name: String,
    pub field: String,
    pub text: String,
}

/// Limits a keyword search to one field of one entity type. The vector index has no fields, so a search with a scope is keyword only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AttrScope {
    pub type_name: String,
    pub field: String,
}

/// One searchable object. Only current objects are in the index, so it has no status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexDoc {
    pub id: ObjectId,
    pub kind: DocKind,
    /// A tested language's code selects its stemmed field. Anything else gets only the neutral fields.
    pub language: Option<String>,
    pub text: String,
    pub source_ids: Vec<ObjectId>,
    pub epistemic: Option<String>,
    pub valid_from_ms: Option<i64>,
    pub valid_to_ms: Option<i64>,
    /// What sort of thing it is within its kind: a segment's media type, a claim's predicate, or an entity's type. Pages and blocks have none.
    pub doc_type: Option<String>,
    /// The trust tiers of the sources behind it. A claim can cite several.
    pub trust_tiers: Vec<String>,
    /// An entity's searchable fields. Other kinds have none.
    pub attrs: Vec<AttrValue>,
}

// Ops are written in small batches on a path that does IO, so the size of the larger variant doesn't matter here.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexOp {
    /// Replaces any document with this id.
    Upsert(IndexDoc),
    /// Does nothing if there's no such document.
    Delete(ObjectId),
}

/// Any word matches, and more matches score higher. A phrase needs its words in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryTerm {
    Word(String),
    Phrase(Vec<String>),
}

/// Empty lists mean no restriction.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LexicalFilter {
    pub kinds: Vec<DocKind>,
    pub source_ids: Vec<ObjectId>,
    pub epistemic: Vec<String>,
    /// Keeps documents valid at this time. A missing bound is open.
    pub valid_at_ms: Option<i64>,
    pub types: Vec<String>,
    pub trust_tiers: Vec<String>,
    pub attr: Option<AttrScope>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LexicalQuery {
    pub terms: Vec<QueryTerm>,
    pub filter: LexicalFilter,
    pub limit: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LexicalHit {
    pub id: ObjectId,
    pub kind: DocKind,
    pub score: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LexicalResults {
    /// Best first, with ties in id order.
    pub hits: Vec<LexicalHit>,
    /// The last event this answer reflects.
    pub watermark: Option<u64>,
}

/// Which generation a write goes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Generation {
    /// The one answering searches.
    Live,
    /// The rebuild in progress, invisible until it's flipped to live.
    Building,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IndexHealth {
    /// No live generation yet.
    Missing,
    /// Built by a version of the backend with another schema or analysis.
    Outdated,
    Corrupt(String),
    Ready {
        watermark: Option<u64>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LexicalError {
    /// There's no generation to use yet, such as during a first build.
    Unavailable(String),
    /// The generation is damaged and has to be rebuilt.
    Corrupt(String),
    /// The caller used the generations out of order.
    Invalid(String),
    Backend(String),
}

impl fmt::Display for LexicalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(m) => write!(f, "keyword index unavailable: {m}"),
            Self::Corrupt(m) => write!(f, "keyword index corrupt: {m}"),
            Self::Invalid(m) => write!(f, "keyword index: {m}"),
            Self::Backend(m) => write!(f, "keyword index backend: {m}"),
        }
    }
}

impl std::error::Error for LexicalError {}
