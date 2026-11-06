use crate::{BlobInfo, BlobStore, BlobStoreError, BlobStored, Clock, Verified};
use memex_types::{BlobHash, VaultId};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::io::{Cursor, Read};
use std::sync::Mutex;

/// The steps of a durable put, for injecting failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlobStep {
    Write,
    Fsync,
    Rename,
    DirFsync,
}

#[derive(Debug, Default)]
struct State {
    blobs: HashMap<VaultId, BTreeMap<BlobHash, (Vec<u8>, i64)>>,
    fail_at: Option<BlobStep>,
}

/// An in-memory BlobStore that can fail at any step. It takes its time from the injected clock.
pub struct MemBlobStore<C: Clock> {
    state: Mutex<State>,
    clock: C,
}

pub(crate) fn sha256(bytes: &[u8]) -> BlobHash {
    let mut text = String::with_capacity(64);
    for b in Sha256::digest(bytes) {
        let _ = write!(text, "{b:02x}");
    }
    BlobHash::parse(&text).expect("sha256 is valid")
}

impl<C: Clock> MemBlobStore<C> {
    pub fn new(clock: C) -> Self {
        Self {
            state: Mutex::new(State::default()),
            clock,
        }
    }

    pub fn clock(&self) -> &C {
        &self.clock
    }

    pub fn fail_next_put_at(&self, step: BlobStep) {
        self.state.lock().expect("lock").fail_at = Some(step);
    }

    /// Flips a stored byte, for testing `verify`.
    pub fn corrupt(&self, vault: &VaultId, hash: &BlobHash) {
        let mut s = self.state.lock().expect("lock");
        if let Some((bytes, _)) = s.blobs.get_mut(vault).and_then(|m| m.get_mut(hash)) {
            bytes[0] ^= 0xff;
        }
    }
}

impl<C: Clock> BlobStore for MemBlobStore<C> {
    type Reader = Cursor<Vec<u8>>;

    fn put(&self, vault: &VaultId, bytes: &mut dyn Read) -> Result<BlobStored, BlobStoreError> {
        let mut buf = vec![];
        bytes.read_to_end(&mut buf).map_err(|e| BlobStoreError::Input(e.to_string()))?;
        let mut s = self.state.lock().expect("lock");
        // A failure at any step leaves nothing visible, as with the real store's temporary file.
        if let Some(step) = s.fail_at.take() {
            return Err(BlobStoreError::Io(format!("injected failure at {step:?}")));
        }
        let hash = sha256(&buf);
        let size = buf.len() as u64;
        let now = self.clock.now_ms();
        let vault_blobs = s.blobs.entry(vault.clone()).or_default();
        let newly_stored = !vault_blobs.contains_key(&hash);
        vault_blobs.entry(hash.clone()).or_insert((buf, now));
        Ok(BlobStored { hash, size, newly_stored })
    }

    fn read(&self, vault: &VaultId, hash: &BlobHash, offset: u64, len: Option<u64>) -> Result<Self::Reader, BlobStoreError> {
        let s = self.state.lock().expect("lock");
        let (bytes, _) = s
            .blobs
            .get(vault)
            .and_then(|m| m.get(hash))
            .ok_or_else(|| BlobStoreError::NotFound { hash: hash.clone() })?;
        let start = (offset as usize).min(bytes.len());
        let end = len.map_or(bytes.len(), |l| start.saturating_add(l as usize).min(bytes.len()));
        Ok(Cursor::new(bytes[start..end].to_vec()))
    }

    fn delete(&self, vault: &VaultId, hash: &BlobHash) -> Result<(), BlobStoreError> {
        let mut s = self.state.lock().expect("lock");
        s.blobs
            .get_mut(vault)
            .and_then(|m| m.remove(hash))
            .map(|_| ())
            .ok_or_else(|| BlobStoreError::NotFound { hash: hash.clone() })
    }

    fn list(&self, vault: &VaultId, after: Option<&BlobHash>, limit: usize) -> Result<Vec<BlobInfo>, BlobStoreError> {
        let s = self.state.lock().expect("lock");
        let Some(m) = s.blobs.get(vault) else { return Ok(vec![]) };
        let items = m.iter().filter(|(h, _)| after.is_none_or(|a| *h > a)).take(limit);
        Ok(items
            .map(|(h, (b, at))| BlobInfo {
                hash: h.clone(),
                size: b.len() as u64,
                stored_at_ms: *at,
            })
            .collect())
    }

    fn purge_incomplete(&self, _: &VaultId) -> Result<u64, BlobStoreError> {
        Ok(0)
    }

    fn delete_vault(&self, vault: &VaultId) -> Result<(), BlobStoreError> {
        self.state.lock().expect("lock").blobs.remove(vault);
        Ok(())
    }

    fn verify(&self, vault: &VaultId, hash: &BlobHash) -> Result<Verified, BlobStoreError> {
        let s = self.state.lock().expect("lock");
        let (bytes, _) = s
            .blobs
            .get(vault)
            .and_then(|m| m.get(hash))
            .ok_or_else(|| BlobStoreError::NotFound { hash: hash.clone() })?;
        let actual = sha256(bytes);
        Ok(if &actual == hash { Verified::Ok } else { Verified::Corrupt { actual } })
    }
}
