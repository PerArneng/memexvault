//! A keyword index in memory. Its matching is simple on purpose: it behaves like a real index only as far as the contract promises.
use crate::LexicalIndex;
use crate::lexical::trim_word;
use memex_types::{Generation, IndexDoc, IndexHealth, IndexOp, LexicalError, LexicalHit, LexicalQuery, LexicalResults, QueryTerm, VaultId};
use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;

#[derive(Debug, Clone, Default)]
struct Gen {
    committed: BTreeMap<String, IndexDoc>,
    pending: BTreeMap<String, IndexDoc>,
    watermark: Option<u64>,
    was_committed: bool,
}

#[derive(Debug, Default)]
struct VaultIndex {
    live: Option<Gen>,
    previous: Option<Gen>,
    building: Option<Gen>,
    corrupt: Option<String>,
}

#[derive(Debug, Default)]
pub struct MemLexical {
    vaults: Mutex<HashMap<VaultId, VaultIndex>>,
    /// Fails the next this many writes, for testing recovery.
    fail_writes: Mutex<usize>,
}

fn fold(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| match c {
            'å' | 'ä' | 'á' | 'à' => 'a',
            'ö' | 'ó' => 'o',
            'ü' | 'ú' => 'u',
            'é' | 'è' => 'e',
            'í' => 'i',
            'ñ' => 'n',
            c => c,
        })
        .collect()
}

fn words(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(trim_word)
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

fn score_term(doc_words: &[String], term: &QueryTerm) -> f32 {
    let word_score = |w: &str| {
        let w = w.to_lowercase();
        let exact = doc_words.iter().filter(|d| **d == w).count() as f32;
        let folded = doc_words.iter().filter(|d| fold(d) == fold(&w)).count() as f32;
        exact * 2.0 + folded
    };
    match term {
        QueryTerm::Word(w) => word_score(w),
        QueryTerm::Phrase(parts) => {
            let parts: Vec<String> = parts.iter().map(|p| fold(p)).collect();
            let folded: Vec<String> = doc_words.iter().map(|d| fold(d)).collect();
            folded.windows(parts.len()).filter(|window| *window == parts.as_slice()).count() as f32
        }
    }
}

fn passes(doc: &IndexDoc, q: &LexicalQuery) -> bool {
    let f = &q.filter;
    (f.kinds.is_empty() || f.kinds.contains(&doc.kind))
        && (f.source_ids.is_empty() || f.source_ids.iter().any(|s| doc.source_ids.contains(s)))
        && (f.epistemic.is_empty() || doc.epistemic.as_ref().is_some_and(|e| f.epistemic.contains(e)))
        && (f.types.is_empty() || doc.doc_type.as_ref().is_some_and(|t| f.types.contains(t)))
        && (f.trust_tiers.is_empty() || f.trust_tiers.iter().any(|t| doc.trust_tiers.contains(t)))
        && f.valid_at_ms
            .is_none_or(|t| doc.valid_from_ms.is_none_or(|from| from <= t) && doc.valid_to_ms.is_none_or(|to| t < to))
}

impl MemLexical {
    pub fn new() -> Self {
        Self::default()
    }

    /// Damages the live generation, as a disk fault would.
    pub fn corrupt(&self, vault: &VaultId, detail: &str) {
        self.vaults.lock().expect("lock").entry(vault.clone()).or_default().corrupt = Some(detail.into());
    }

    pub fn fail_next_writes(&self, n: usize) {
        *self.fail_writes.lock().expect("lock") = n;
    }

    fn with_gen<T>(&self, vault: &VaultId, generation: Generation, f: impl FnOnce(&mut Gen) -> T) -> Result<T, LexicalError> {
        let mut vaults = self.vaults.lock().expect("lock");
        let v = vaults.entry(vault.clone()).or_default();
        let target = match generation {
            Generation::Live => v.live.as_mut(),
            Generation::Building => v.building.as_mut(),
        };
        target.map(f).ok_or_else(|| LexicalError::Invalid(format!("no {generation:?} generation")))
    }
}

impl LexicalIndex for MemLexical {
    fn health(&self, vault: &VaultId, _deep: bool) -> Result<IndexHealth, LexicalError> {
        let vaults = self.vaults.lock().expect("lock");
        let Some(v) = vaults.get(vault) else { return Ok(IndexHealth::Missing) };
        if let Some(detail) = &v.corrupt {
            return Ok(IndexHealth::Corrupt(detail.clone()));
        }
        Ok(v.live.as_ref().map_or(IndexHealth::Missing, |g| IndexHealth::Ready { watermark: g.watermark }))
    }

    fn watermark(&self, vault: &VaultId, generation: Generation) -> Result<Option<u64>, LexicalError> {
        self.with_gen(vault, generation, |g| g.watermark)
    }

    fn begin_build(&self, vault: &VaultId) -> Result<(), LexicalError> {
        self.vaults.lock().expect("lock").entry(vault.clone()).or_default().building = Some(Gen::default());
        Ok(())
    }

    fn write(&self, vault: &VaultId, generation: Generation, ops: &[IndexOp]) -> Result<(), LexicalError> {
        {
            let mut fails = self.fail_writes.lock().expect("lock");
            if *fails > 0 {
                *fails -= 1;
                return Err(LexicalError::Backend("the disk is full".into()));
            }
        }
        self.with_gen(vault, generation, |g| {
            for op in ops {
                match op {
                    IndexOp::Upsert(doc) => {
                        g.pending.insert(doc.id.0.clone(), doc.clone());
                    }
                    IndexOp::Delete(id) => {
                        g.pending.remove(&id.0);
                    }
                }
            }
        })
    }

    fn commit(&self, vault: &VaultId, generation: Generation, watermark: Option<u64>) -> Result<(), LexicalError> {
        self.with_gen(vault, generation, |g| {
            g.committed = g.pending.clone();
            g.watermark = watermark;
            g.was_committed = true;
        })
    }

    fn flip(&self, vault: &VaultId) -> Result<(), LexicalError> {
        let mut vaults = self.vaults.lock().expect("lock");
        let v = vaults.entry(vault.clone()).or_default();
        match v.building.take() {
            Some(g) if g.was_committed => {
                v.previous = v.live.replace(g);
                v.corrupt = None;
                Ok(())
            }
            other => {
                v.building = other;
                Err(LexicalError::Invalid("no committed generation to flip to".into()))
            }
        }
    }

    fn rollback(&self, vault: &VaultId) -> Result<(), LexicalError> {
        let mut vaults = self.vaults.lock().expect("lock");
        let v = vaults.entry(vault.clone()).or_default();
        let Some(previous) = v.previous.take() else {
            return Err(LexicalError::Invalid("no earlier generation to go back to".into()));
        };
        v.live = Some(previous);
        Ok(())
    }

    fn search(&self, vault: &VaultId, query: &LexicalQuery) -> Result<LexicalResults, LexicalError> {
        let vaults = self.vaults.lock().expect("lock");
        let v = vaults.get(vault).ok_or_else(|| LexicalError::Unavailable("no index yet".into()))?;
        if let Some(detail) = &v.corrupt {
            return Err(LexicalError::Corrupt(detail.clone()));
        }
        let g = v.live.as_ref().ok_or_else(|| LexicalError::Unavailable("no index yet".into()))?;
        let mut hits: Vec<LexicalHit> = g
            .committed
            .values()
            .filter(|d| passes(d, query))
            .filter_map(|d| {
                let doc_words = words(&d.text);
                let score: f32 = query.terms.iter().map(|t| score_term(&doc_words, t)).sum();
                (score > 0.0).then(|| LexicalHit {
                    id: d.id.clone(),
                    kind: d.kind,
                    score,
                })
            })
            .collect();
        hits.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.id.cmp(&b.id)));
        hits.truncate(query.limit);
        Ok(LexicalResults { hits, watermark: g.watermark })
    }

    fn delete_vault(&self, vault: &VaultId) -> Result<(), LexicalError> {
        self.vaults.lock().expect("lock").remove(vault);
        Ok(())
    }
}
