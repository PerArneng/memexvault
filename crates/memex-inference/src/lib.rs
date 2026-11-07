//! Embeddings on ONNX Runtime. Running a model isn't logic, so this stays thin: the text handling and pooling that can be tested without a model are pure functions.
// It reads model files, and only when first asked to embed or load.
#![allow(clippy::disallowed_methods)]
use memex_core::{Embedder, Reranker};
use memex_types::{EmbedError, EmbedPurpose, ModelInfo, RerankError};
use ort::session::{Session, SessionInputValue};
use ort::value::Tensor;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use tokenizers::{PaddingParams, Tokenizer, TruncationParams};

pub const E5_SMALL: &str = "multilingual-e5-small";
pub const E5_SMALL_DIMENSION: u32 = 384;
pub const MODEL_FILE: &str = "model.onnx";
pub const TOKENIZER_FILE: &str = "tokenizer.json";
pub const RERANK_MODEL: &str = "mmarco-mMiniLMv2-L12-H384-v1";
pub const RERANK_MODEL_FILE: &str = "model_qint8_arm64.onnx";
/// Segments are 100 to 400 tokens, and spec 005 measured this limit.
const MAX_TOKENS: usize = 256;

/// e5 tells a question from the text that answers it by a prefix.
pub fn e5_input(purpose: EmbedPurpose, text: &str) -> String {
    match purpose {
        EmbedPurpose::Query => format!("query: {text}"),
        EmbedPurpose::Passage => format!("passage: {text}"),
    }
}

/// Averages each text's token vectors, ignoring padding, and scales the result to length 1, so an inner product is a cosine. `tokens` is texts × length × dimension, in one flat run.
pub fn mean_pool(tokens: &[f32], texts: usize, length: usize, dimension: usize, mask: &[i64]) -> Vec<Vec<f32>> {
    (0..texts)
        .map(|i| {
            let mut sum = vec![0f32; dimension];
            let mut count = 0f32;
            for j in 0..length {
                if mask[i * length + j] == 1 {
                    let at = (i * length + j) * dimension;
                    for (s, t) in sum.iter_mut().zip(&tokens[at..at + dimension]) {
                        *s += t;
                    }
                    count += 1.0;
                }
            }
            let norm = sum.iter().map(|x| x * x).sum::<f32>().sqrt();
            if count == 0.0 || norm == 0.0 {
                return sum;
            }
            sum.iter().map(|x| x / norm).collect()
        })
        .collect()
}

struct Loaded {
    tokenizer: Tokenizer,
    /// Runs need exclusive use of a session. Passages get their own, so background embedding never holds up a question.
    passage: Mutex<Session>,
    queries: Vec<Mutex<Session>>,
}

/// e5-small, loaded on first use, since commands that never embed shouldn't pay for it.
pub struct OnnxEmbedder {
    dir: PathBuf,
    threads: usize,
    query_sessions: usize,
    loaded: OnceLock<Result<Loaded, String>>,
    next_query: AtomicUsize,
}

/// A free session if there is one, and otherwise a turn on the next in line, so a query never queues behind a busy session while another sits idle.
fn take_session<'a>(sessions: &'a [Mutex<Session>], next: &AtomicUsize) -> Result<MutexGuard<'a, Session>, String> {
    let start = next.fetch_add(1, Ordering::Relaxed);
    for i in 0..sessions.len() {
        if let Ok(free) = sessions[(start + i) % sessions.len()].try_lock() {
            return Ok(free);
        }
    }
    sessions[start % sessions.len()].lock().map_err(|_| "a session's lock is poisoned".to_string())
}

fn session(path: &Path, threads: usize) -> Result<Session, String> {
    let failed = |e: String| format!("can't load {}: {e}", path.display());
    let builder = Session::builder().map_err(|e| failed(e.to_string()))?;
    let mut builder = builder.with_intra_threads(threads).map_err(|e| failed(e.to_string()))?;
    builder.commit_from_file(path).map_err(|e| failed(e.to_string()))
}

fn load(dir: &Path, threads: usize, query_sessions: usize) -> Result<Loaded, String> {
    let mut tokenizer = Tokenizer::from_file(dir.join(TOKENIZER_FILE)).map_err(|e| format!("can't load the tokenizer: {e}"))?;
    tokenizer.with_padding(Some(PaddingParams::default()));
    tokenizer
        .with_truncation(Some(TruncationParams {
            max_length: MAX_TOKENS,
            ..Default::default()
        }))
        .map_err(|e| e.to_string())?;
    let model = dir.join(MODEL_FILE);
    let passage = Mutex::new(session(&model, threads)?);
    let queries = (0..query_sessions.max(1))
        .map(|_| session(&model, threads).map(Mutex::new))
        .collect::<Result<_, _>>()?;
    Ok(Loaded { tokenizer, passage, queries })
}

impl OnnxEmbedder {
    pub fn new(dir: impl Into<PathBuf>, threads: usize, query_sessions: usize) -> Self {
        Self {
            dir: dir.into(),
            threads,
            query_sessions,
            loaded: OnceLock::new(),
            next_query: AtomicUsize::new(0),
        }
    }

    /// Loads the model now, so the first question doesn't wait for it and a bad file stops startup.
    pub fn load(&self) -> Result<(), String> {
        self.loaded().map(|_| ())
    }

    fn loaded(&self) -> Result<&Loaded, String> {
        self.loaded
            .get_or_init(|| load(&self.dir, self.threads, self.query_sessions))
            .as_ref()
            .map_err(Clone::clone)
    }

    fn run(sessions: &[Mutex<Session>], next: &AtomicUsize, tokenizer: &Tokenizer, purpose: EmbedPurpose, texts: &[String]) -> Result<Vec<Vec<f32>>, String> {
        let inputs: Vec<String> = texts.iter().map(|t| e5_input(purpose, t)).collect();
        let encodings = tokenizer.encode_batch(inputs, true).map_err(|e| e.to_string())?;
        let (n, length) = (encodings.len(), encodings.first().map_or(0, tokenizers::Encoding::len));
        let flat =
            |field: &dyn Fn(&tokenizers::Encoding) -> Vec<u32>| -> Vec<i64> { encodings.iter().flat_map(|e| field(e).into_iter().map(i64::from)).collect() };
        let ids = flat(&|e| e.get_ids().to_vec());
        let mask = flat(&|e| e.get_attention_mask().to_vec());
        let types = flat(&|e| e.get_type_ids().to_vec());
        let mut session = take_session(sessions, next)?;
        let names: Vec<String> = session.inputs().iter().map(|i| i.name().to_string()).collect();
        let mut values: Vec<(String, SessionInputValue)> = Vec::with_capacity(names.len());
        for name in names {
            let data = match name.as_str() {
                "input_ids" => ids.clone(),
                "attention_mask" => mask.clone(),
                "token_type_ids" => types.clone(),
                other => return Err(format!("the model asks for an unknown input: {other}")),
            };
            let tensor = Tensor::from_array(([n, length], data)).map_err(|e| e.to_string())?;
            values.push((name, tensor.into()));
        }
        let outputs = session.run(values).map_err(|e| e.to_string())?;
        let (shape, tokens) = outputs[0].try_extract_tensor::<f32>().map_err(|e| e.to_string())?;
        let dimension = shape.get(2).copied().ok_or("the model's output has no token dimension")? as usize;
        Ok(mean_pool(tokens, n, length, dimension, &mask))
    }
}

impl Embedder for OnnxEmbedder {
    fn active(&self) -> ModelInfo {
        ModelInfo {
            name: E5_SMALL.into(),
            dimension: E5_SMALL_DIMENSION,
        }
    }

    fn model(&self, name: &str) -> Option<ModelInfo> {
        (name == E5_SMALL).then(|| self.active())
    }

    fn embed(&self, model: &str, purpose: EmbedPurpose, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
        if model != E5_SMALL {
            return Err(EmbedError::UnknownModel(model.into()));
        }
        if texts.is_empty() {
            return Ok(vec![]);
        }
        let loaded = self.loaded().map_err(EmbedError::Failed)?;
        let sessions = match purpose {
            EmbedPurpose::Passage => std::slice::from_ref(&loaded.passage),
            EmbedPurpose::Query => loaded.queries.as_slice(),
        };
        Self::run(sessions, &self.next_query, &loaded.tokenizer, purpose, texts).map_err(EmbedError::Failed)
    }
}

/// Spec 005 measured this limit for reranking.
const RERANK_MAX_TOKENS: usize = 128;

struct LoadedReranker {
    tokenizer: Tokenizer,
    sessions: Vec<Mutex<Session>>,
}

fn load_reranker(dir: &Path, threads: usize, sessions: usize) -> Result<LoadedReranker, String> {
    let mut tokenizer = Tokenizer::from_file(dir.join(TOKENIZER_FILE)).map_err(|e| format!("can't load the tokenizer: {e}"))?;
    tokenizer.with_padding(Some(PaddingParams::default()));
    tokenizer
        .with_truncation(Some(TruncationParams {
            max_length: RERANK_MAX_TOKENS,
            ..Default::default()
        }))
        .map_err(|e| e.to_string())?;
    let model = dir.join(RERANK_MODEL_FILE);
    let sessions = (0..sessions.max(1))
        .map(|_| session(&model, threads).map(Mutex::new))
        .collect::<Result<_, _>>()?;
    Ok(LoadedReranker { tokenizer, sessions })
}

/// `mmarco` int8, loaded on first use, since reranking is opt-in and many servers never pay for it.
pub struct OnnxReranker {
    dir: PathBuf,
    threads: usize,
    sessions: usize,
    loaded: OnceLock<Result<LoadedReranker, String>>,
    next: AtomicUsize,
}

impl OnnxReranker {
    pub fn new(dir: impl Into<PathBuf>, threads: usize, sessions: usize) -> Self {
        Self {
            dir: dir.into(),
            threads,
            sessions,
            loaded: OnceLock::new(),
            next: AtomicUsize::new(0),
        }
    }

    fn score(&self, query: &str, passages: &[&str]) -> Result<Vec<f32>, String> {
        let loaded = self
            .loaded
            .get_or_init(|| load_reranker(&self.dir, self.threads, self.sessions))
            .as_ref()
            .map_err(Clone::clone)?;
        let pairs: Vec<(&str, &str)> = passages.iter().map(|p| (query, *p)).collect();
        let encodings = loaded.tokenizer.encode_batch(pairs, true).map_err(|e| e.to_string())?;
        let (n, length) = (encodings.len(), encodings.first().map_or(0, tokenizers::Encoding::len));
        let flat =
            |field: &dyn Fn(&tokenizers::Encoding) -> Vec<u32>| -> Vec<i64> { encodings.iter().flat_map(|e| field(e).into_iter().map(i64::from)).collect() };
        let ids = flat(&|e| e.get_ids().to_vec());
        let mask = flat(&|e| e.get_attention_mask().to_vec());
        let types = flat(&|e| e.get_type_ids().to_vec());
        let mut session = take_session(&loaded.sessions, &self.next)?;
        let names: Vec<String> = session.inputs().iter().map(|i| i.name().to_string()).collect();
        let mut values: Vec<(String, SessionInputValue)> = Vec::with_capacity(names.len());
        for name in names {
            let data = match name.as_str() {
                "input_ids" => ids.clone(),
                "attention_mask" => mask.clone(),
                "token_type_ids" => types.clone(),
                other => return Err(format!("the model asks for an unknown input: {other}")),
            };
            let tensor = Tensor::from_array(([n, length], data)).map_err(|e| e.to_string())?;
            values.push((name, tensor.into()));
        }
        let outputs = session.run(values).map_err(|e| e.to_string())?;
        let (shape, logits) = outputs[0].try_extract_tensor::<f32>().map_err(|e| e.to_string())?;
        let columns = shape.get(1).copied().unwrap_or(1).max(1) as usize;
        Ok(logits.chunks(columns).filter_map(|row| row.last().copied()).collect())
    }
}

impl Reranker for OnnxReranker {
    fn model(&self) -> String {
        RERANK_MODEL.into()
    }

    fn rerank(&self, query: &str, passages: &[&str]) -> Result<Vec<f32>, RerankError> {
        if passages.is_empty() {
            return Ok(vec![]);
        }
        self.score(query, passages).map_err(RerankError::Failed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_reranker_model_is_a_failure_not_a_panic() {
        let r = OnnxReranker::new("/nonexistent", 1, 1);
        assert!(matches!(r.rerank("q", &["p"]), Err(RerankError::Failed(_))));
        assert_eq!(r.rerank("q", &[]), Ok(vec![]));
    }

    #[test]
    fn inputs_get_the_prefix_for_their_purpose() {
        assert_eq!(e5_input(EmbedPurpose::Query, "x"), "query: x");
        assert_eq!(e5_input(EmbedPurpose::Passage, "x"), "passage: x");
    }

    #[test]
    fn pooling_ignores_padding_and_gives_unit_length() {
        // Two texts of length 2 in 2 dimensions. The second has one real token.
        let tokens = [3.0, 0.0, 3.0, 8.0, 0.0, 2.0, 100.0, 100.0];
        let pooled = mean_pool(&tokens, 2, 2, 2, &[1, 1, 1, 0]);
        assert!((pooled[0][0] - 0.6).abs() < 1e-6 && (pooled[0][1] - 0.8).abs() < 1e-6, "{pooled:?}");
        assert_eq!(pooled[1], vec![0.0, 1.0], "the padded token counts for nothing");
    }

    #[test]
    fn an_all_padding_or_zero_text_stays_finite() {
        let pooled = mean_pool(&[0.0, 0.0], 1, 1, 2, &[0]);
        assert_eq!(pooled[0], vec![0.0, 0.0]);
    }

    #[test]
    fn a_missing_model_is_a_failure_not_a_panic() {
        let e = OnnxEmbedder::new("/nonexistent", 1, 1);
        assert!(matches!(e.embed(E5_SMALL, EmbedPurpose::Query, &["x".into()]), Err(EmbedError::Failed(_))));
        assert!(e.load().is_err());
    }

    #[test]
    fn it_offers_only_its_own_model() {
        let e = OnnxEmbedder::new("/nonexistent", 1, 1);
        assert!(e.model("other").is_none());
        assert_eq!(e.model(E5_SMALL).unwrap().dimension, 384);
        assert_eq!(
            e.embed("other", EmbedPurpose::Query, &["x".into()]),
            Err(EmbedError::UnknownModel("other".into()))
        );
        assert_eq!(e.embed(E5_SMALL, EmbedPurpose::Query, &[]), Ok(vec![]));
    }
}
