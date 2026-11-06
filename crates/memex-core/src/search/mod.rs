//! Hybrid search: plans a query, gathers candidates from the indexes, fuses and fences them, and fits the result into an evidence package. Everything but the edge traits runs in memory.
pub mod assemble;
pub mod budget;
pub mod fusion;
pub mod history;
pub mod plan;
pub mod relevance;
mod run;

pub(crate) use run::vector_candidates;
pub use run::{SearchEdgesRef, run_search};

use memex_types::RerankError;

/// A cross-encoder, which reads a query with each passage together. It's opt-in, as it's far slower than the rest of a search.
pub trait Reranker {
    /// One score per passage, in order, higher meaning more relevant.
    fn rerank(&self, query: &str, passages: &[&str]) -> Result<Vec<f32>, RerankError>;
}

/// How many of the best candidates get reranked. Spec 005 measured 133 ms for 20 at 128 tokens, and no local model met 250 ms for 50. See spec 055.
pub const RERANK_TOP: usize = 20;
