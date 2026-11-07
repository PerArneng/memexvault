//! Edge implementations of the core traits. Thin by design.
// The one place allowed to read the system clock.
#![allow(clippy::disallowed_methods)]
use memex_core::{Clock, Seed};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub struct SystemClock {
    start: Instant,
}

impl Default for SystemClock {
    fn default() -> Self {
        Self { start: Instant::now() }
    }
}

impl Clock for SystemClock {
    fn now_ms(&self) -> i64 {
        SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or(Duration::ZERO).as_millis() as i64
    }

    fn monotonic(&self) -> Duration {
        self.start.elapsed()
    }
}

/// Random numbers from the operating system, good enough for secrets such as tokens.
pub struct SystemSeed;

impl Seed for SystemSeed {
    fn next_u64(&self) -> u64 {
        // The OS's random source failing isn't recoverable, and a guessable token would be worse than stopping.
        getrandom::u64().expect("the operating system's random source failed")
    }
}

/// The LLM behind the core trait. It only translates.
pub struct AdkAgentRunner(pub memex_agent_adk::AdkAgent);

impl memex_core::AgentRunner for AdkAgentRunner {
    fn model(&self) -> String {
        self.0.model().to_string()
    }

    fn run(&self, req: &memex_types::AgentRequest) -> Result<memex_types::AgentReply, memex_types::AgentError> {
        self.0.run(req)
    }
}

/// The endpoint settings name. Settings have already checked each provider has what it needs.
pub fn llm_endpoint(llm: &memex_core::LlmSettings) -> Result<memex_agent_adk::LlmEndpoint, String> {
    match llm.provider.as_str() {
        "gemini" => Ok(memex_agent_adk::LlmEndpoint::Gemini {
            api_key: llm.api_key.clone().ok_or("MEMEX_LLM_API_KEY isn't set")?,
            model: llm.model.clone(),
        }),
        "openai" => Ok(memex_agent_adk::LlmEndpoint::OpenAi {
            base_url: llm.base_url.clone().ok_or("MEMEX_LLM_BASE_URL isn't set")?,
            api_key: llm.api_key.clone(),
            model: llm.model.clone(),
        }),
        other => Err(format!("unknown LLM provider {other}")),
    }
}

/// The default BlobStore: `memex-blob-fs` behind the core trait. It only translates.
pub struct FsBlobStore<O: memex_blob_fs::FsOps>(pub memex_blob_fs::FsBlobs<O>);

fn blob_error(hash: Option<&memex_types::BlobHash>, vault: &memex_types::VaultId, e: memex_blob_fs::FsBlobsError) -> memex_core::BlobStoreError {
    use memex_blob_fs::FsBlobsError as F;
    use memex_core::BlobStoreError as B;
    match e {
        F::NotFound => match hash {
            Some(h) => B::NotFound { hash: h.clone() },
            None => B::Io("not found".into()),
        },
        F::InvalidVault => B::InvalidVault { vault: vault.clone() },
        F::Input(e) => B::Input(e.to_string()),
        F::Io(e) => B::Io(e.to_string()),
    }
}

impl<O: memex_blob_fs::FsOps> memex_core::BlobStore for FsBlobStore<O> {
    type Reader = std::io::Take<std::fs::File>;

    fn put(&self, vault: &memex_types::VaultId, bytes: &mut dyn std::io::Read) -> Result<memex_core::BlobStored, memex_core::BlobStoreError> {
        let (hash, size, newly_stored) = self.0.put(vault, bytes).map_err(|e| blob_error(None, vault, e))?;
        Ok(memex_core::BlobStored { hash, size, newly_stored })
    }

    fn read(
        &self,
        vault: &memex_types::VaultId,
        hash: &memex_types::BlobHash,
        offset: u64,
        len: Option<u64>,
    ) -> Result<Self::Reader, memex_core::BlobStoreError> {
        self.0.read(vault, hash, offset, len).map_err(|e| blob_error(Some(hash), vault, e))
    }

    fn delete(&self, vault: &memex_types::VaultId, hash: &memex_types::BlobHash) -> Result<(), memex_core::BlobStoreError> {
        self.0.delete(vault, hash).map_err(|e| blob_error(Some(hash), vault, e))
    }

    fn list(
        &self,
        vault: &memex_types::VaultId,
        after: Option<&memex_types::BlobHash>,
        limit: usize,
    ) -> Result<Vec<memex_core::BlobInfo>, memex_core::BlobStoreError> {
        let items = self.0.list(vault, after, limit).map_err(|e| blob_error(None, vault, e))?;
        Ok(items
            .into_iter()
            .map(|i| memex_core::BlobInfo {
                hash: i.hash,
                size: i.size,
                stored_at_ms: i.stored_at_ms,
            })
            .collect())
    }

    fn purge_incomplete(&self, vault: &memex_types::VaultId) -> Result<u64, memex_core::BlobStoreError> {
        self.0.purge_tmp(vault).map_err(|e| blob_error(None, vault, e))
    }

    fn delete_vault(&self, vault: &memex_types::VaultId) -> Result<(), memex_core::BlobStoreError> {
        self.0.delete_vault(vault).map_err(|e| blob_error(None, vault, e))
    }

    fn verify(&self, vault: &memex_types::VaultId, hash: &memex_types::BlobHash) -> Result<memex_core::Verified, memex_core::BlobStoreError> {
        match self.0.verify(vault, hash).map_err(|e| blob_error(Some(hash), vault, e))? {
            memex_blob_fs::VerifyResult::Ok => Ok(memex_core::Verified::Ok),
            memex_blob_fs::VerifyResult::Corrupt { actual } => Ok(memex_core::Verified::Corrupt { actual }),
        }
    }
}

/// The default LexicalIndex: `memex-lexical` behind the core trait. Its types are already the trait's, so this only forwards.
pub struct TantivyIndex(pub memex_lexical::TantivyLexical);

impl memex_core::LexicalIndex for TantivyIndex {
    fn health(&self, vault: &memex_types::VaultId, deep: bool) -> Result<memex_types::IndexHealth, memex_types::LexicalError> {
        self.0.health(vault, deep)
    }

    fn watermark(&self, vault: &memex_types::VaultId, generation: memex_types::Generation) -> Result<Option<u64>, memex_types::LexicalError> {
        self.0.watermark(vault, generation)
    }

    fn begin_build(&self, vault: &memex_types::VaultId) -> Result<(), memex_types::LexicalError> {
        self.0.begin_build(vault)
    }

    fn write(&self, vault: &memex_types::VaultId, generation: memex_types::Generation, ops: &[memex_types::IndexOp]) -> Result<(), memex_types::LexicalError> {
        self.0.write(vault, generation, ops)
    }

    fn commit(&self, vault: &memex_types::VaultId, generation: memex_types::Generation, watermark: Option<u64>) -> Result<(), memex_types::LexicalError> {
        self.0.commit(vault, generation, watermark)
    }

    fn flip(&self, vault: &memex_types::VaultId) -> Result<(), memex_types::LexicalError> {
        self.0.flip(vault)
    }

    fn rollback(&self, vault: &memex_types::VaultId) -> Result<(), memex_types::LexicalError> {
        self.0.rollback(vault)
    }

    fn search(&self, vault: &memex_types::VaultId, query: &memex_types::LexicalQuery) -> Result<memex_types::LexicalResults, memex_types::LexicalError> {
        self.0.search(vault, query)
    }

    fn delete_vault(&self, vault: &memex_types::VaultId) -> Result<(), memex_types::LexicalError> {
        self.0.delete_vault(vault)
    }

    fn reconcile(
        &self,
        vault: &memex_types::VaultId,
        kind: memex_types::DocKind,
        wanted: &[memex_types::ObjectId],
    ) -> Result<Option<memex_core::lexical::IndexContents>, memex_types::LexicalError> {
        let looked_for: std::collections::HashSet<&str> = wanted.iter().map(|i| i.0.as_str()).collect();
        let mut out = memex_core::lexical::IndexContents::default();
        let live = self.0.for_each_id(vault, kind, |id| {
            memex_core::audit::add_to_digest(&mut out.digest, id);
            if looked_for.contains(id) {
                out.present.insert(memex_types::ObjectId(id.into()));
            }
        })?;
        Ok(live.then_some(out))
    }
}

/// The default VectorIndex: `memex-vector` behind the core trait. Its types are already the trait's, so this only forwards.
pub struct PgVectorIndex(pub memex_vector::PgVectors);

impl memex_core::VectorIndex for PgVectorIndex {
    fn live_mark(&self, vault: &memex_types::VaultId) -> Result<Option<u64>, memex_types::VectorError> {
        self.0.live_mark(vault)
    }

    fn health(&self, vault: &memex_types::VaultId) -> Result<memex_types::VectorHealth, memex_types::VectorError> {
        self.0.health(vault)
    }

    fn watermark(&self, vault: &memex_types::VaultId, generation: memex_types::Generation) -> Result<Option<u64>, memex_types::VectorError> {
        self.0.watermark(vault, generation)
    }

    fn begin_build(&self, vault: &memex_types::VaultId, model: &memex_types::ModelInfo) -> Result<(), memex_types::VectorError> {
        self.0.begin_build(vault, model)
    }

    fn apply(
        &self,
        vault: &memex_types::VaultId,
        generation: memex_types::Generation,
        ops: &[memex_types::VectorOp],
        watermark: Option<u64>,
    ) -> Result<(), memex_types::VectorError> {
        self.0.apply(vault, generation, ops, watermark)
    }

    fn flip(&self, vault: &memex_types::VaultId) -> Result<(), memex_types::VectorError> {
        self.0.flip(vault)
    }

    fn rollback(&self, vault: &memex_types::VaultId) -> Result<(), memex_types::VectorError> {
        self.0.rollback(vault)
    }

    fn search(&self, vault: &memex_types::VaultId, query: &memex_types::VectorQuery) -> Result<memex_types::LexicalResults, memex_types::VectorError> {
        self.0.search(vault, query)
    }

    fn embeddings(
        &self,
        vault: &memex_types::VaultId,
        model: &str,
        hashes: &[String],
    ) -> Result<std::collections::HashMap<String, Vec<f32>>, memex_types::VectorError> {
        self.0.embeddings(vault, model, hashes)
    }

    fn store_embeddings(&self, vault: &memex_types::VaultId, model: &str, rows: &[(String, Vec<f32>)]) -> Result<(), memex_types::VectorError> {
        self.0.store_embeddings(vault, model, rows)
    }

    /// The vectors live in the vault's schema, which deleting the vault in the store has already dropped.
    fn delete_vault(&self, vault: &memex_types::VaultId) -> Result<(), memex_types::VectorError> {
        self.0.forget_mark(vault);
        Ok(())
    }

    fn reconcile(
        &self,
        vault: &memex_types::VaultId,
        kind: memex_types::DocKind,
        wanted: &[memex_types::ObjectId],
    ) -> Result<Option<memex_core::lexical::IndexContents>, memex_types::VectorError> {
        Ok(self
            .0
            .reconcile(vault, kind.as_str(), wanted)?
            .map(|(count, xor, present)| memex_core::lexical::IndexContents {
                digest: memex_types::IdDigest { count, xor },
                present: present.into_iter().map(memex_types::ObjectId).collect(),
            }))
    }
}
