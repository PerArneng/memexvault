//! What a search is asked and what it answers: the evidence package.
use crate::rendition::Rect;
use crate::{DocKind, LexicalFilter, ObjectId};

/// Which signals answer. Hybrid is the product. The other two exist for comparison and debugging.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchMode {
    Hybrid,
    Lexical,
    Vector,
}

impl SearchMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "hybrid" => Some(Self::Hybrid),
            "lexical" => Some(Self::Lexical),
            "vector" => Some(Self::Vector),
            _ => None,
        }
    }
}

/// A past state of the vault.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AsOf {
    /// Just after this event.
    Seq(u64),
    /// Just after the last event at or before this time, in milliseconds since 1970.
    Time(i64),
}

/// Whether a search follows Edges from its best candidates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expand {
    /// As the planner's rules decide.
    Auto,
    On,
    Off,
}

impl Expand {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "auto" => Some(Self::Auto),
            "on" => Some(Self::On),
            "off" => Some(Self::Off),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchRequest {
    pub text: String,
    pub mode: SearchMode,
    pub filter: LexicalFilter,
    /// The most ranked items per page.
    pub limit: usize,
    /// The most estimated tokens per page.
    pub budget_tokens: usize,
    pub cursor: Option<String>,
    pub as_of: Option<AsOf>,
    /// Refuse to answer until the indexes have seen this event.
    pub min_seq: Option<u64>,
    pub rerank: bool,
    pub debug: bool,
    pub facets: bool,
    pub expand: Expand,
}

impl SearchRequest {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            mode: SearchMode::Hybrid,
            filter: LexicalFilter::default(),
            limit: 20,
            budget_tokens: 4000,
            cursor: None,
            as_of: None,
            min_seq: None,
            rerank: false,
            debug: false,
            facets: false,
            expand: Expand::Auto,
        }
    }
}

/// The route the planner chose. Rules decide it, never a model.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryPath {
    /// The query is an object's id, so the store answers.
    Exact,
    /// Mostly part numbers, codes and the like, where literal matches count for more.
    Identifier,
    Natural,
    Filtered,
    /// A past state, which only the log holds.
    AsOf,
}

impl QueryPath {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Exact => "exact",
            Self::Identifier => "identifier",
            Self::Natural => "natural",
            Self::Filtered => "filtered",
            Self::AsOf => "as_of",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemKind {
    Claim,
    Segment,
    Entity,
    /// The segments around a hit under the same heading, for context.
    Section,
    Page,
    Block,
}

impl ItemKind {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Claim => "claim",
            Self::Segment => "segment",
            Self::Entity => "entity",
            Self::Section => "section",
            Self::Page => "page",
            Self::Block => "block",
        }
    }

    pub const fn of(kind: DocKind) -> Self {
        match kind {
            DocKind::Claim => Self::Claim,
            DocKind::Segment => Self::Segment,
            DocKind::Entity => Self::Entity,
            DocKind::Page => Self::Page,
            DocKind::Block => Self::Block,
        }
    }
}

/// Whether a citation leads to a place in the source. Every citation says, so nothing has to be assumed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AnchorStatus {
    Located,
    /// It was located once, and re-anchoring on a new rendition failed.
    Broken,
    /// It never had a place, such as a segment recorded without a rendition.
    None,
}

impl AnchorStatus {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Located => "located",
            Self::Broken => "broken",
            Self::None => "none",
        }
    }
}

/// Where an item's text comes from. Page rectangles appear only on claims, whose anchors store them. A segment gives offsets and pages, because rectangles would need its word table on every search.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Citation {
    pub source_id: Option<ObjectId>,
    pub source_name: Option<String>,
    pub source_sha256: Option<String>,
    pub trust_tier: Option<String>,
    pub segment_id: Option<ObjectId>,
    pub evidence_id: Option<ObjectId>,
    pub status: AnchorStatus,
    pub rendition_id: Option<ObjectId>,
    /// UTF-8 byte offsets into the rendition's canonical text.
    pub start: Option<u64>,
    pub end: Option<u64>,
    pub page_from: Option<u32>,
    pub page_to: Option<u32>,
    /// One per line, with its 0-based page.
    pub rects: Vec<(u32, Rect)>,
    /// Verbatim. Absent when the item's text is itself the quote.
    pub quote: Option<String>,
    /// Where it plays, when it comes from a recording.
    pub time: Option<crate::rendition::TimeSpan>,
}

/// A claim that disagrees with this one. Both stay, and the reader decides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub claim_id: ObjectId,
    pub text: String,
    pub epistemic: Option<String>,
    pub source_ids: Vec<ObjectId>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EvidenceItem {
    pub id: ObjectId,
    pub kind: ItemKind,
    pub text: String,
    pub heading: Option<String>,
    pub language: Option<String>,
    pub epistemic: Option<String>,
    pub trust_tiers: Vec<String>,
    pub valid_from_ms: Option<i64>,
    pub valid_to_ms: Option<i64>,
    /// The fused rank score, or the reranker's when it ran. Absent on context sections.
    pub score: Option<f32>,
    pub estimated_tokens: usize,
    pub citations: Vec<Citation>,
    pub conflicts: Vec<Conflict>,
    /// The wiki page a page or block item belongs to.
    pub page_id: Option<ObjectId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BudgetReport {
    pub limit: usize,
    pub used: usize,
    /// More items were found than fit. `next_cursor` continues.
    pub truncated: bool,
    /// Set when not even the next item fits: what it would have needed.
    pub smallest_item_needs: Option<usize>,
}

/// Each signal's part in one item.
#[derive(Debug, Clone, PartialEq)]
pub struct SignalScores {
    pub id: ObjectId,
    pub lexical_rank: Option<usize>,
    pub lexical_score: Option<f32>,
    pub vector_rank: Option<usize>,
    pub vector_score: Option<f32>,
    pub fused: f32,
    pub rerank_score: Option<f32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchDebug {
    pub lexical_weight: f32,
    pub vector_weight: f32,
    /// How many query terms went to the keyword index, and how many were cut for length.
    pub lexical_terms: usize,
    pub dropped_terms: usize,
    pub best_vector_score: Option<f32>,
    pub term_coverage: Option<f32>,
    pub candidates_before_fence: usize,
    pub candidates_after_fence: usize,
    /// Candidates that following Edges added.
    pub expanded: usize,
    pub signals: Vec<SignalScores>,
    /// Milliseconds per stage, in order.
    pub timings_ms: Vec<(String, f64)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FacetCount {
    pub value: String,
    pub count: u64,
}

/// Counts over the candidates that passed the fence, not over the whole vault, so they cost nothing to speak of.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Facets {
    pub kind: Vec<FacetCount>,
    pub doc_type: Vec<FacetCount>,
    pub source_id: Vec<FacetCount>,
    pub trust_tier: Vec<FacetCount>,
    pub epistemic: Vec<FacetCount>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct EvidencePackage {
    pub path: QueryPath,
    /// Claims and entities first, then segments, then context sections.
    pub items: Vec<EvidenceItem>,
    /// Nothing here looks like an answer. The caller shouldn't treat these items as one.
    pub low_relevance: bool,
    /// Plain words on anything unusual about this answer.
    pub notices: Vec<String>,
    pub budget: BudgetReport,
    pub next_cursor: Option<String>,
    /// The last event every index used here reflects.
    pub watermark: Option<u64>,
    /// The event a past state was read at, absent for the present or for a time before the first event.
    pub as_of: Option<u64>,
    /// Signals that couldn't answer, so the result rests on the others.
    pub degraded: Vec<String>,
    pub debug: Option<SearchDebug>,
    pub facets: Option<Facets>,
}

/// What a cross-encoder promises: one score per passage, in order, higher meaning more relevant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RerankError {
    /// May work next time, or needs `models fetch`.
    Failed(String),
}

impl std::fmt::Display for RerankError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Failed(m) => write!(f, "reranking failed: {m}"),
        }
    }
}

impl std::error::Error for RerankError {}
