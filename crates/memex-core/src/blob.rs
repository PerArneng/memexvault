use memex_types::{BlobHash, VaultId};
use std::collections::HashSet;
use std::fmt;
use std::io::Read;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlobStoreError {
    NotFound {
        hash: BlobHash,
    },
    /// The vault id can't be used safely by this backend.
    InvalidVault {
        vault: VaultId,
    },
    /// The source of the bytes failed, for example a client that disconnected. Nothing was stored.
    Input(String),
    /// A storage failure, such as a full disk. Nothing became visible, and temporary data was cleaned up.
    Io(String),
}

impl fmt::Display for BlobStoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotFound { hash } => write!(f, "blob {hash} not found"),
            Self::InvalidVault { vault } => write!(f, "invalid vault id {vault:?}"),
            Self::Input(m) => write!(f, "reading input: {m}"),
            Self::Io(m) => write!(f, "blob storage: {m}"),
        }
    }
}

impl std::error::Error for BlobStoreError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobStored {
    pub hash: BlobHash,
    pub size: u64,
    /// False when these bytes were already stored.
    pub newly_stored: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlobInfo {
    pub hash: BlobHash,
    pub size: u64,
    pub stored_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verified {
    Ok,
    Corrupt { actual: BlobHash },
}

/// Content-addressed bytes. No path appears here, so a remote backend fits behind the same trait.
pub trait BlobStore {
    type Reader: Read;

    /// Streams bytes in. When it returns they are durable and visible under their hash. An existing hash is a no-op.
    fn put(&self, vault: &VaultId, bytes: &mut dyn Read) -> Result<BlobStored, BlobStoreError>;
    /// Streams `len` bytes from `offset`, or to the end when `len` is `None`. A range past the end returns what exists.
    fn read(&self, vault: &VaultId, hash: &BlobHash, offset: u64, len: Option<u64>) -> Result<Self::Reader, BlobStoreError>;
    fn delete(&self, vault: &VaultId, hash: &BlobHash) -> Result<(), BlobStoreError>;
    /// Blobs in hash order, after the given hash, at most `limit`.
    fn list(&self, vault: &VaultId, after: Option<&BlobHash>, limit: usize) -> Result<Vec<BlobInfo>, BlobStoreError>;
    /// Removes leftovers of puts that a crash interrupted. Call it while holding the vault's writer lease, so no put is running.
    fn purge_incomplete(&self, vault: &VaultId) -> Result<u64, BlobStoreError>;
    /// Removes all of a vault's blobs. A vault with none is fine.
    fn delete_vault(&self, vault: &VaultId) -> Result<(), BlobStoreError>;
    /// Re-hashes the stored bytes.
    fn verify(&self, vault: &VaultId, hash: &BlobHash) -> Result<Verified, BlobStoreError>;
}

/// The orphan sweep's decision. Pure: a referenced blob is never swept, and a young one may still be about to get its reference.
pub fn sweepable(blob: &BlobInfo, referenced: &HashSet<BlobHash>, now_ms: i64, grace_ms: i64) -> bool {
    !referenced.contains(&blob.hash) && now_ms.saturating_sub(blob.stored_at_ms) >= grace_ms
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hash(c: char) -> BlobHash {
        BlobHash::parse(&c.to_string().repeat(64)).unwrap()
    }

    fn info(at: i64) -> BlobInfo {
        BlobInfo {
            hash: hash('a'),
            size: 1,
            stored_at_ms: at,
        }
    }

    #[test]
    fn old_unreferenced_blob_is_swept() {
        assert!(sweepable(&info(0), &HashSet::new(), 1000, 1000));
    }

    #[test]
    fn young_blob_is_kept() {
        assert!(!sweepable(&info(1), &HashSet::new(), 1000, 1000));
    }

    #[test]
    fn referenced_blob_is_kept_however_old() {
        assert!(!sweepable(&info(0), &HashSet::from([hash('a')]), i64::MAX, 0));
    }

    #[test]
    fn a_clock_behind_the_file_keeps_it() {
        assert!(!sweepable(&info(5000), &HashSet::new(), 1000, 1000));
    }
}
