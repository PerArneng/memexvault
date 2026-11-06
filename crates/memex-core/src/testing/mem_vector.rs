//! A vector index and an embedder in memory. Search is exact, which is what the contract asks for at the sizes tests use.
use crate::{Embedder, VectorIndex};
use memex_types::{
    EmbedError, EmbedPurpose, Generation, LexicalFilter, LexicalHit, LexicalResults, ModelInfo, VaultId, VectorDoc, VectorError, VectorHealth, VectorOp,
    VectorQuery,
};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Clone)]
struct Gen {
    model: ModelInfo,
    docs: BTreeMap<String, VectorDoc>,
    watermark: Option<u64>,
}

#[derive(Debug, Default)]
struct VaultIndex {
    live: Option<Gen>,
    previous: Option<Gen>,
    building: Option<Gen>,
    embeddings: HashMap<(String, String), Vec<f32>>,
}

#[derive(Debug, Default)]
pub struct MemVector {
    vaults: Mutex<HashMap<VaultId, VaultIndex>>,
    fail_applies: Mutex<usize>,
}

/// Whether a document passes a filter. Shared so the fake and the real index mean the same by it.
pub fn passes(doc: &VectorDoc, f: &LexicalFilter) -> bool {
    (f.kinds.is_empty() || f.kinds.contains(&doc.kind))
        && (f.source_ids.is_empty() || doc.source_ids.iter().any(|s| f.source_ids.contains(s)))
        && (f.epistemic.is_empty() || doc.epistemic.as_ref().is_some_and(|e| f.epistemic.contains(e)))
        && (f.types.is_empty() || doc.doc_type.as_ref().is_some_and(|t| f.types.contains(t)))
        && (f.trust_tiers.is_empty() || f.trust_tiers.iter().any(|t| doc.trust_tiers.contains(t)))
        && f.valid_at_ms
            .is_none_or(|t| doc.valid_from_ms.is_none_or(|from| from <= t) && doc.valid_to_ms.is_none_or(|to| t < to))
}

impl MemVector {
    pub fn new() -> Self {
        Self::default()
    }

    /// Fails the next this many applies, for testing recovery.
    pub fn fail_next_applies(&self, n: usize) {
        *self.fail_applies.lock().expect("lock") = n;
    }

    pub fn stored_embeddings(&self, vault: &VaultId) -> usize {
        self.vaults.lock().expect("lock").get(vault).map_or(0, |v| v.embeddings.len())
    }

    pub fn building_model(&self, vault: &VaultId) -> Option<ModelInfo> {
        self.vaults
            .lock()
            .expect("lock")
            .get(vault)
            .and_then(|v| v.building.as_ref().map(|g| g.model.clone()))
    }
}

impl VectorIndex for MemVector {
    fn health(&self, vault: &VaultId) -> Result<VectorHealth, VectorError> {
        let vaults = self.vaults.lock().expect("lock");
        Ok(match vaults.get(vault).and_then(|v| v.live.as_ref()) {
            Some(g) => VectorHealth::Ready {
                model: g.model.clone(),
                watermark: g.watermark,
            },
            None => VectorHealth::Missing,
        })
    }

    fn watermark(&self, vault: &VaultId, generation: Generation) -> Result<Option<u64>, VectorError> {
        let vaults = self.vaults.lock().expect("lock");
        let v = vaults.get(vault);
        Ok(match generation {
            Generation::Live => v.and_then(|v| v.live.as_ref()).and_then(|g| g.watermark),
            Generation::Building => v.and_then(|v| v.building.as_ref()).and_then(|g| g.watermark),
        })
    }

    fn begin_build(&self, vault: &VaultId, model: &ModelInfo) -> Result<(), VectorError> {
        self.vaults.lock().expect("lock").entry(vault.clone()).or_default().building = Some(Gen {
            model: model.clone(),
            docs: BTreeMap::new(),
            watermark: None,
        });
        Ok(())
    }

    fn apply(&self, vault: &VaultId, generation: Generation, ops: &[VectorOp], watermark: Option<u64>) -> Result<(), VectorError> {
        {
            let mut fail = self.fail_applies.lock().expect("lock");
            if *fail > 0 {
                *fail -= 1;
                return Err(VectorError::Backend("a scripted failure".into()));
            }
        }
        let mut vaults = self.vaults.lock().expect("lock");
        let v = vaults.entry(vault.clone()).or_default();
        let g = match generation {
            Generation::Live => v.live.as_mut(),
            Generation::Building => v.building.as_mut(),
        }
        .ok_or_else(|| VectorError::Invalid("no such generation".into()))?;
        for op in ops {
            if let VectorOp::Upsert(d) = op
                && d.vector.len() as u32 != g.model.dimension
            {
                return Err(VectorError::WrongDimension {
                    expected: g.model.dimension,
                    got: d.vector.len() as u32,
                });
            }
        }
        for op in ops {
            match op {
                VectorOp::Upsert(d) => {
                    g.docs.insert(d.id.0.clone(), d.clone());
                }
                VectorOp::Delete(id) => {
                    g.docs.remove(&id.0);
                }
            }
        }
        g.watermark = watermark;
        Ok(())
    }

    fn flip(&self, vault: &VaultId) -> Result<(), VectorError> {
        let mut vaults = self.vaults.lock().expect("lock");
        let v = vaults.entry(vault.clone()).or_default();
        let building = v.building.take().ok_or_else(|| VectorError::Invalid("nothing is being built".into()))?;
        v.previous = v.live.replace(building);
        Ok(())
    }

    fn rollback(&self, vault: &VaultId) -> Result<(), VectorError> {
        let mut vaults = self.vaults.lock().expect("lock");
        let v = vaults.entry(vault.clone()).or_default();
        let previous = v.previous.take().ok_or_else(|| VectorError::Invalid("there's no earlier generation".into()))?;
        v.live = Some(previous);
        Ok(())
    }

    fn search(&self, vault: &VaultId, query: &VectorQuery) -> Result<LexicalResults, VectorError> {
        let vaults = self.vaults.lock().expect("lock");
        let g = vaults
            .get(vault)
            .and_then(|v| v.live.as_ref())
            .ok_or_else(|| VectorError::Unavailable("no generation yet".into()))?;
        if g.model.name != query.model {
            return Err(VectorError::ModelMismatch {
                live: g.model.name.clone(),
                asked: query.model.clone(),
            });
        }
        if query.vector.len() as u32 != g.model.dimension {
            return Err(VectorError::WrongDimension {
                expected: g.model.dimension,
                got: query.vector.len() as u32,
            });
        }
        let mut hits: Vec<LexicalHit> = g
            .docs
            .values()
            .filter(|d| passes(d, &query.filter))
            .map(|d| LexicalHit {
                id: d.id.clone(),
                kind: d.kind,
                score: d.vector.iter().zip(&query.vector).map(|(a, b)| a * b).sum(),
            })
            .collect();
        hits.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.id.cmp(&b.id)));
        hits.truncate(query.limit);
        Ok(LexicalResults { hits, watermark: g.watermark })
    }

    fn embeddings(&self, vault: &VaultId, model: &str, hashes: &[String]) -> Result<HashMap<String, Vec<f32>>, VectorError> {
        let vaults = self.vaults.lock().expect("lock");
        let Some(v) = vaults.get(vault) else {
            return Ok(HashMap::new());
        };
        Ok(hashes
            .iter()
            .filter_map(|h| v.embeddings.get(&(model.to_string(), h.clone())).map(|e| (h.clone(), e.clone())))
            .collect())
    }

    fn store_embeddings(&self, vault: &VaultId, model: &str, rows: &[(String, Vec<f32>)]) -> Result<(), VectorError> {
        let mut vaults = self.vaults.lock().expect("lock");
        let v = vaults.entry(vault.clone()).or_default();
        for (h, e) in rows {
            v.embeddings.insert((model.to_string(), h.clone()), e.clone());
        }
        Ok(())
    }

    fn delete_vault(&self, vault: &VaultId) -> Result<(), VectorError> {
        self.vaults.lock().expect("lock").remove(vault);
        Ok(())
    }
}

/// A unit vector from a text's hash: the same text always gets the same one, in any dimension.
pub fn fake_vector(text: &str, dimension: u32) -> Vec<f32> {
    let mut out = Vec::with_capacity(dimension as usize);
    let mut block = Sha256::digest(text.as_bytes());
    let mut i = 0;
    while out.len() < dimension as usize {
        if i == block.len() {
            block = Sha256::digest(block);
            i = 0;
        }
        out.push(f32::from(block[i]) - 127.5);
        i += 1;
    }
    let norm = out.iter().map(|x| x * x).sum::<f32>().sqrt().max(f32::MIN_POSITIVE);
    out.iter().map(|x| x / norm).collect()
}

/// A deterministic embedder that counts its work and fails on request. Only the exact text matters, so a query for a passage's own text scores 1.
#[derive(Debug)]
pub struct FakeEmbedder {
    active: Mutex<ModelInfo>,
    models: Mutex<Vec<ModelInfo>>,
    passages: AtomicU64,
    queries: AtomicU64,
    fail_next: Mutex<usize>,
    wrong_dimension: Mutex<bool>,
}

impl Default for FakeEmbedder {
    fn default() -> Self {
        Self::new("fake-a", 8)
    }
}

impl FakeEmbedder {
    pub fn new(name: &str, dimension: u32) -> Self {
        let model = ModelInfo { name: name.into(), dimension };
        Self {
            active: Mutex::new(model.clone()),
            models: Mutex::new(vec![model]),
            passages: AtomicU64::new(0),
            queries: AtomicU64::new(0),
            fail_next: Mutex::new(0),
            wrong_dimension: Mutex::new(false),
        }
    }

    /// Offers another model and makes it the active one, keeping the others.
    pub fn activate(&self, name: &str, dimension: u32) {
        let model = ModelInfo { name: name.into(), dimension };
        self.models.lock().expect("lock").push(model.clone());
        *self.active.lock().expect("lock") = model;
    }

    /// Stops offering a model, as if its files were gone.
    pub fn remove(&self, name: &str) {
        self.models.lock().expect("lock").retain(|m| m.name != name);
    }

    pub fn passages_embedded(&self) -> u64 {
        self.passages.load(Ordering::SeqCst)
    }

    pub fn queries_embedded(&self) -> u64 {
        self.queries.load(Ordering::SeqCst)
    }

    pub fn fail_next(&self, n: usize) {
        *self.fail_next.lock().expect("lock") = n;
    }

    /// Makes every later passage vector one element short.
    pub fn return_wrong_dimension(&self) {
        *self.wrong_dimension.lock().expect("lock") = true;
    }
}

impl Embedder for FakeEmbedder {
    fn active(&self) -> ModelInfo {
        self.active.lock().expect("lock").clone()
    }

    fn model(&self, name: &str) -> Option<ModelInfo> {
        self.models.lock().expect("lock").iter().find(|m| m.name == name).cloned()
    }

    fn embed(&self, model: &str, purpose: EmbedPurpose, texts: &[String]) -> Result<Vec<Vec<f32>>, EmbedError> {
        let info = self.model(model).ok_or_else(|| EmbedError::UnknownModel(model.into()))?;
        {
            let mut fail = self.fail_next.lock().expect("lock");
            if *fail > 0 {
                *fail -= 1;
                return Err(EmbedError::Failed("a scripted failure".into()));
            }
        }
        let counter = match purpose {
            EmbedPurpose::Query => &self.queries,
            EmbedPurpose::Passage => &self.passages,
        };
        counter.fetch_add(texts.len() as u64, Ordering::SeqCst);
        let short = purpose == EmbedPurpose::Passage && *self.wrong_dimension.lock().expect("lock");
        Ok(texts
            .iter()
            .map(|t| {
                let mut v = fake_vector(&format!("{model}\n{t}"), info.dimension);
                if short {
                    v.pop();
                }
                v
            })
            .collect())
    }
}
