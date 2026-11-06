//! What a vector index stores and answers, and what an embedder promises. No engine or runtime type appears here.
use crate::{DocKind, LexicalFilter, ObjectId};
use std::fmt;

/// A model's vectors can only be compared with vectors from the same model, so the name travels with them.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ModelInfo {
    pub name: String,
    pub dimension: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VectorDoc {
    pub id: ObjectId,
    pub kind: DocKind,
    pub source_ids: Vec<ObjectId>,
    pub epistemic: Option<String>,
    pub valid_from_ms: Option<i64>,
    pub valid_to_ms: Option<i64>,
    pub doc_type: Option<String>,
    pub trust_tiers: Vec<String>,
    pub vector: Vec<f32>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum VectorOp {
    /// Replaces any document with this id.
    Upsert(VectorDoc),
    /// Does nothing if there's no such document.
    Delete(ObjectId),
}

#[derive(Debug, Clone, PartialEq)]
pub struct VectorQuery {
    /// The model that made `vector`. A query for any other model than the live one's is refused.
    pub model: String,
    pub vector: Vec<f32>,
    pub filter: LexicalFilter,
    pub limit: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VectorHealth {
    /// No live generation yet.
    Missing,
    Ready {
        model: ModelInfo,
        watermark: Option<u64>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VectorError {
    /// There's no generation to use yet, such as during a first build.
    Unavailable(String),
    /// A query from another model than the live generation's. Its scores would mean nothing.
    ModelMismatch {
        live: String,
        asked: String,
    },
    WrongDimension {
        expected: u32,
        got: u32,
    },
    /// The caller used the generations out of order.
    Invalid(String),
    Backend(String),
}

impl fmt::Display for VectorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unavailable(m) => write!(f, "vector index unavailable: {m}"),
            Self::ModelMismatch { live, asked } => write!(f, "vector index holds {live}, and was asked about {asked}"),
            Self::WrongDimension { expected, got } => write!(f, "vector index expects {expected} dimensions, and got {got}"),
            Self::Invalid(m) => write!(f, "vector index: {m}"),
            Self::Backend(m) => write!(f, "vector index backend: {m}"),
        }
    }
}

impl std::error::Error for VectorError {}

/// Some models embed a question and the text that answers it differently.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmbedPurpose {
    Query,
    Passage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EmbedError {
    /// Trying again can't help.
    UnknownModel(String),
    /// Trying again can't help: the model's output doesn't match what it declares.
    WrongDimension { expected: u32, got: u32 },
    /// May work next time.
    Failed(String),
}

impl fmt::Display for EmbedError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnknownModel(m) => write!(f, "no embedding model called {m}"),
            Self::WrongDimension { expected, got } => write!(f, "embedding model declares {expected} dimensions, and returned {got}"),
            Self::Failed(m) => write!(f, "embedding failed: {m}"),
        }
    }
}

impl std::error::Error for EmbedError {}
