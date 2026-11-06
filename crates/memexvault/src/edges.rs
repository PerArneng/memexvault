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
