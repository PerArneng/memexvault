//! The vector index's and embedder's traits, and the pure policy around them: what text gets embedded, and which embeddings a set of objects still needs.
use crate::{Snapshot, StoreError};
use memex_types::{
    DocKind, EmbedError, EmbedPurpose, IndexDoc, IndexOp, JobRow, LexicalResults, ModelInfo, ObjectId, Row, RowKey, Table, VaultId, VectorDoc, VectorError,
    VectorHealth, VectorOp, VectorQuery,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fmt::Write as _;

/// Turns text into vectors. It offers one active model, which new content uses, and may offer others that a generation still depends on.
pub trait Embedder {
    fn active(&self) -> ModelInfo;
    fn model(&self, name: &str) -> Option<ModelInfo>;
    /// One vector per text, in order, each of the model's dimension.
    fn embed(&self, model: &str, purpose: EmbedPurpose, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError>;
}

/// A vector index, and the stored embeddings it's built from. Only the projector writes to the index.
pub trait VectorIndex {
    fn health(&self, vault: &VaultId) -> Result<VectorHealth, VectorError>;
    /// The last event the generation covers. `Ok(None)` also means there's no such generation.
    fn watermark(&self, vault: &VaultId, generation: memex_types::Generation) -> Result<Option<u64>, VectorError>;
    /// Starts an empty building generation for this model, discarding any unfinished one.
    fn begin_build(&self, vault: &VaultId, model: &ModelInfo) -> Result<(), VectorError>;
    /// The writes and the new watermark, atomically and durably. Idempotent per object id.
    fn apply(&self, vault: &VaultId, generation: memex_types::Generation, ops: &[VectorOp], watermark: Option<u64>) -> Result<(), VectorError>;
    /// Makes the building generation live atomically. The old one stays for rollback.
    fn flip(&self, vault: &VaultId) -> Result<(), VectorError>;
    /// Goes back to the generation before the last flip.
    fn rollback(&self, vault: &VaultId) -> Result<(), VectorError>;
    /// Best first, by inner product, with returned ties in id order.
    fn search(&self, vault: &VaultId, query: &VectorQuery) -> Result<LexicalResults, VectorError>;
    /// The stored vectors among these content hashes. One call however many hashes.
    fn embeddings(&self, vault: &VaultId, model: &str, hashes: &[String]) -> Result<HashMap<String, Vec<f32>>, VectorError>;
    /// Idempotent per model and hash.
    fn store_embeddings(&self, vault: &VaultId, model: &str, rows: &[(String, Vec<f32>)]) -> Result<(), VectorError>;
    fn delete_vault(&self, vault: &VaultId) -> Result<(), VectorError>;
}

pub const EMBED_JOB: &str = "embed";
/// Texts per job. A job holds a worker for the length of a batch, which should stay well under the job lease.
pub const EMBED_BATCH: usize = 32;
pub const EMBED_ATTEMPTS: u32 = 5;

/// Which objects a job embeds. It names them rather than their text, since their current text is what matters when it runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbedInput {
    pub model: String,
    pub docs: Vec<EmbedDocRef>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbedDocRef {
    pub kind: DocKind,
    pub id: ObjectId,
}

/// One object that has no stored embedding yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MissingEmbedding {
    pub doc: EmbedDocRef,
    pub hash: String,
}

/// A job to create, under a key that stops the same work being queued twice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbedRequest {
    pub input: EmbedInput,
    pub dedupe_key: String,
}

fn sha256_hex(text: &str) -> String {
    let mut out = String::with_capacity(64);
    for b in Sha256::digest(text.as_bytes()) {
        let _ = write!(out, "{b:02x}");
    }
    out
}

/// The key of the embedding store, together with the model.
pub fn content_hash(text: &str) -> String {
    sha256_hex(text)
}

/// What the model reads. The document's title gives a passage the context it lacks on its own. It's deterministic, so identical input hashes the same.
pub fn embedding_text(doc: &IndexDoc, title: Option<&str>) -> String {
    match title {
        Some(t) if !t.is_empty() => format!("{t}\n{}", doc.text),
        _ => doc.text.clone(),
    }
}

/// An object with its text and hash, ready to look up or embed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddableDoc {
    pub doc: IndexDoc,
    pub text: String,
    pub hash: String,
}

/// Adds titles and hashes to documents. Claims quote their sources and carry no title. One store call at most.
pub fn embeddable<S: Snapshot>(snap: &S, docs: Vec<IndexDoc>) -> Result<Vec<EmbeddableDoc>, StoreError> {
    let ids: Vec<ObjectId> = docs
        .iter()
        .filter(|d| d.kind == DocKind::Segment)
        .flat_map(|d| d.source_ids.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let keys: Vec<RowKey> = ids
        .iter()
        .map(|id| RowKey {
            table: Table::Source,
            id: id.clone(),
        })
        .collect();
    let sources = if keys.is_empty() { vec![] } else { snap.get_many(&keys)? };
    let titles: HashMap<ObjectId, String> = ids
        .into_iter()
        .zip(sources)
        .filter_map(|(id, row)| match row {
            Some(Row::Source(s)) => Some((id, s.name)),
            _ => None,
        })
        .collect();
    Ok(docs
        .into_iter()
        .map(|doc| {
            let title = (doc.kind == DocKind::Segment)
                .then(|| doc.source_ids.first().and_then(|id| titles.get(id)))
                .flatten();
            let text = embedding_text(&doc, title.map(String::as_str));
            let hash = content_hash(&text);
            EmbeddableDoc { doc, text, hash }
        })
        .collect())
}

/// What a set of current objects means for an index of this model.
#[derive(Debug, Default, PartialEq)]
pub struct VectorPlan {
    pub ops: Vec<VectorOp>,
    pub missing: Vec<MissingEmbedding>,
}

fn vector_doc(doc: IndexDoc, vector: Vec<f32>) -> VectorDoc {
    VectorDoc {
        id: doc.id,
        kind: doc.kind,
        source_ids: doc.source_ids,
        epistemic: doc.epistemic,
        valid_from_ms: doc.valid_from_ms,
        valid_to_ms: doc.valid_to_ms,
        vector,
    }
}

/// Reuses stored embeddings. An object without one leaves the index until its job is done, so it can't answer with a vector of its old text.
pub fn plan_ops<S: Snapshot, V: VectorIndex>(snap: &S, index: &V, vault: &VaultId, model: &str, ops: Vec<IndexOp>) -> Result<VectorPlan, crate::ProjectError> {
    let mut plan = VectorPlan::default();
    let mut docs = vec![];
    for op in ops {
        match op {
            IndexOp::Upsert(doc) => docs.push(doc),
            IndexOp::Delete(id) => plan.ops.push(VectorOp::Delete(id)),
        }
    }
    let docs = embeddable(snap, docs)?;
    if docs.is_empty() {
        return Ok(plan);
    }
    let hashes: Vec<String> = docs.iter().map(|d| d.hash.clone()).collect::<BTreeSet<_>>().into_iter().collect();
    let stored = index.embeddings(vault, model, &hashes)?;
    for d in docs {
        match stored.get(&d.hash) {
            Some(vector) => plan.ops.push(VectorOp::Upsert(vector_doc(d.doc, vector.clone()))),
            None => {
                plan.ops.push(VectorOp::Delete(d.doc.id.clone()));
                plan.missing.push(MissingEmbedding {
                    doc: EmbedDocRef {
                        kind: d.doc.kind,
                        id: d.doc.id,
                    },
                    hash: d.hash,
                });
            }
        }
    }
    Ok(plan)
}

/// Groups what's missing into jobs. The key covers each object's current hash, so a changed text gets a new job and a repeat of the same work doesn't.
pub fn embed_requests(model: &str, missing: &[MissingEmbedding]) -> Vec<EmbedRequest> {
    missing
        .chunks(EMBED_BATCH)
        .map(|chunk| {
            let mut key = String::new();
            for m in chunk {
                let _ = writeln!(key, "{}:{}:{}", m.doc.kind.as_str(), m.doc.id.0, m.hash);
            }
            EmbedRequest {
                input: EmbedInput {
                    model: model.into(),
                    docs: chunk.iter().map(|m| m.doc.clone()).collect(),
                },
                dedupe_key: format!("embed:{model}:{}", sha256_hex(&key)),
            }
        })
        .collect()
}

/// The objects a finished embed job has made searchable, or nothing for any other job.
pub fn embedded_by(job: &JobRow) -> Vec<EmbedDocRef> {
    if job.kind != EMBED_JOB || job.state != memex_types::JobState::Done {
        return vec![];
    }
    serde_json::from_value::<EmbedInput>(job.input.clone()).map_or(vec![], |i| i.docs)
}

/// Embeds what's still missing for these documents, once per distinct text. Returns how many texts it embedded.
pub fn embed_missing<V: VectorIndex, E: Embedder>(
    index: &V,
    embedder: &E,
    vault: &VaultId,
    model: &str,
    docs: Vec<EmbeddableDoc>,
) -> Result<usize, EmbedJobError> {
    let texts: BTreeMap<String, String> = docs.into_iter().map(|d| (d.hash, d.text)).collect();
    let hashes: Vec<String> = texts.keys().cloned().collect();
    if hashes.is_empty() {
        return Ok(0);
    }
    let stored = index.embeddings(vault, model, &hashes).map_err(EmbedJobError::retry)?;
    let missing: Vec<(String, String)> = texts.into_iter().filter(|(h, _)| !stored.contains_key(h)).collect();
    if missing.is_empty() {
        return Ok(0);
    }
    let info = embedder
        .model(model)
        .ok_or_else(|| EmbedJobError::permanent(EmbedError::UnknownModel(model.into())))?;
    let inputs: Vec<String> = missing.iter().map(|(_, t)| t.clone()).collect();
    let vectors = embedder.embed(model, EmbedPurpose::Passage, &inputs).map_err(|e| match e {
        EmbedError::Failed(_) => EmbedJobError::retry(e),
        other => EmbedJobError::permanent(other),
    })?;
    if vectors.len() != missing.len() {
        return Err(EmbedJobError::retry(EmbedError::Failed(format!(
            "asked for {} vectors, and got {}",
            missing.len(),
            vectors.len()
        ))));
    }
    if let Some(wrong) = vectors.iter().find(|v| v.len() as u32 != info.dimension) {
        return Err(EmbedJobError::permanent(EmbedError::WrongDimension {
            expected: info.dimension,
            got: wrong.len() as u32,
        }));
    }
    let rows: Vec<(String, Vec<f32>)> = missing.into_iter().map(|(h, _)| h).zip(vectors).collect();
    let count = rows.len();
    index.store_embeddings(vault, model, &rows).map_err(EmbedJobError::retry)?;
    Ok(count)
}

/// Why an embed job failed, and whether another attempt could help.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbedJobError {
    pub message: String,
    pub retryable: bool,
}

impl EmbedJobError {
    pub fn retry(e: impl std::fmt::Display) -> Self {
        Self {
            message: e.to_string(),
            retryable: true,
        }
    }

    pub fn permanent(e: impl std::fmt::Display) -> Self {
        Self {
            message: e.to_string(),
            retryable: false,
        }
    }
}

/// Checks a vector query's text. The embedder reads free text, so unlike keywords it has no syntax to reject.
pub fn check_query_text(text: &str) -> Result<(), crate::lexical::QueryError> {
    if text.len() > crate::lexical::MAX_QUERY_BYTES {
        return Err(crate::lexical::QueryError::TooLong {
            limit: crate::lexical::MAX_QUERY_BYTES,
        });
    }
    if text.trim().is_empty() {
        return Err(crate::lexical::QueryError::Empty);
    }
    Ok(())
}
