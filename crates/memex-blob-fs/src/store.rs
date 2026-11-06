use crate::fs_ops::FsOps;
use memex_types::{BlobHash, VaultId};
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{self, Read, Seek, SeekFrom, Take};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::UNIX_EPOCH;

const BUFFER: usize = 64 * 1024;

#[derive(Debug)]
pub enum FsBlobsError {
    NotFound,
    InvalidVault,
    Input(io::Error),
    Io(io::Error),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredInfo {
    pub hash: BlobHash,
    pub size: u64,
    pub stored_at_ms: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VerifyResult {
    Ok,
    Corrupt { actual: BlobHash },
}

pub struct FsBlobs<O: FsOps> {
    root: PathBuf,
    ops: O,
    counter: AtomicU64,
}

fn hex_digest(hasher: Sha256) -> BlobHash {
    let mut text = String::with_capacity(64);
    for b in hasher.finalize() {
        let _ = write!(text, "{b:02x}");
    }
    BlobHash::parse(&text).expect("sha256 is valid")
}

/// Vault ids become one folder name, so anything that could leave that folder is refused.
fn vault_dir_name(vault: &VaultId) -> Result<&str, FsBlobsError> {
    let name = vault.0.as_str();
    let safe = !name.is_empty() && name != "." && name != ".." && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_' || b == b'.');
    if safe { Ok(name) } else { Err(FsBlobsError::InvalidVault) }
}

impl<O: FsOps> FsBlobs<O> {
    pub fn new(root: impl Into<PathBuf>, ops: O) -> Self {
        Self {
            root: root.into(),
            ops,
            counter: AtomicU64::new(0),
        }
    }

    fn vault_root(&self, vault: &VaultId) -> Result<PathBuf, FsBlobsError> {
        Ok(self.root.join("vaults").join(vault_dir_name(vault)?))
    }

    fn final_path(&self, vault: &VaultId, hash: &BlobHash) -> Result<PathBuf, FsBlobsError> {
        let h = hash.as_str();
        Ok(self.vault_root(vault)?.join("blobs").join(&h[..2]).join(&h[2..4]).join(h))
    }

    /// Removes leftovers from a crash. Call it when taking the vault's writer lease, so no other put can be running.
    pub fn purge_tmp(&self, vault: &VaultId) -> Result<u64, FsBlobsError> {
        let tmp = self.vault_root(vault)?.join("tmp");
        let entries = match fs::read_dir(&tmp) {
            Ok(e) => e,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(0),
            Err(e) => return Err(FsBlobsError::Io(e)),
        };
        let mut removed = 0;
        for entry in entries {
            fs::remove_file(entry.map_err(FsBlobsError::Io)?.path()).map_err(FsBlobsError::Io)?;
            removed += 1;
        }
        Ok(removed)
    }

    /// Hash while writing to `tmp/`, fsync, rename, fsync the directories. An error removes the temporary file, so nothing partial is left.
    pub fn put(&self, vault: &VaultId, bytes: &mut dyn Read) -> Result<(BlobHash, u64, bool), FsBlobsError> {
        let root = self.vault_root(vault)?;
        let tmp_dir = root.join("tmp");
        fs::create_dir_all(&tmp_dir).map_err(FsBlobsError::Io)?;
        let tmp = tmp_dir.join(format!("put-{}-{}", std::process::id(), self.counter.fetch_add(1, Ordering::Relaxed)));
        let result = self.put_via(vault, bytes, &tmp);
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        result
    }

    fn put_via(&self, vault: &VaultId, bytes: &mut dyn Read, tmp: &Path) -> Result<(BlobHash, u64, bool), FsBlobsError> {
        let mut file = File::create(tmp).map_err(FsBlobsError::Io)?;
        let (mut hasher, mut size, mut buf) = (Sha256::new(), 0u64, vec![0u8; BUFFER]);
        loop {
            let n = match bytes.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(FsBlobsError::Input(e)),
            };
            hasher.update(&buf[..n]);
            self.ops.write(&mut file, &buf[..n]).map_err(FsBlobsError::Io)?;
            size += n as u64;
        }
        self.ops.fsync(&file).map_err(FsBlobsError::Io)?;
        drop(file);
        let hash = hex_digest(hasher);
        let target = self.final_path(vault, &hash)?;
        if target.exists() {
            fs::remove_file(tmp).map_err(FsBlobsError::Io)?;
            return Ok((hash, size, false));
        }
        let shard2 = target.parent().expect("has parent");
        fs::create_dir_all(shard2).map_err(FsBlobsError::Io)?;
        self.ops.rename(tmp, &target).map_err(FsBlobsError::Io)?;
        // The rename and any new shard folders survive a power cut only once their parents are synced.
        let shard1 = shard2.parent().expect("has parent");
        let blobs = shard1.parent().expect("has parent");
        for dir in [shard2, shard1, blobs] {
            self.ops.fsync_dir(dir).map_err(FsBlobsError::Io)?;
        }
        Ok((hash, size, true))
    }

    pub fn read(&self, vault: &VaultId, hash: &BlobHash, offset: u64, len: Option<u64>) -> Result<Take<File>, FsBlobsError> {
        let mut file = File::open(self.final_path(vault, hash)?).map_err(not_found_or_io)?;
        file.seek(SeekFrom::Start(offset)).map_err(FsBlobsError::Io)?;
        Ok(file.take(len.unwrap_or(u64::MAX)))
    }

    pub fn delete(&self, vault: &VaultId, hash: &BlobHash) -> Result<(), FsBlobsError> {
        let path = self.final_path(vault, hash)?;
        fs::remove_file(&path).map_err(not_found_or_io)?;
        self.ops.fsync_dir(path.parent().expect("has parent")).map_err(FsBlobsError::Io)
    }

    /// Blobs in hash order. Shards are sorted, so `limit` stops the walk early and `after` skips earlier shards.
    pub fn list(&self, vault: &VaultId, after: Option<&BlobHash>, limit: usize) -> Result<Vec<StoredInfo>, FsBlobsError> {
        let blobs = self.vault_root(vault)?.join("blobs");
        let mut out = vec![];
        let after_str = after.map(BlobHash::as_str);
        for s1 in sorted_names(&blobs)? {
            if after_str.is_some_and(|a| s1.as_str() < &a[..2]) {
                continue;
            }
            for s2 in sorted_names(&blobs.join(&s1))? {
                if after_str.is_some_and(|a| format!("{s1}{s2}").as_str() < &a[..4]) {
                    continue;
                }
                let dir = blobs.join(&s1).join(&s2);
                for name in sorted_names(&dir)? {
                    if after_str.is_some_and(|a| name.as_str() <= a) {
                        continue;
                    }
                    if out.len() == limit {
                        return Ok(out);
                    }
                    // A name that isn't a hash isn't ours, so it's not listed or swept.
                    let Ok(hash) = BlobHash::parse(&name) else { continue };
                    let meta = fs::metadata(dir.join(&name)).map_err(FsBlobsError::Io)?;
                    let stored_at_ms = meta
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                        .map_or(0, |d| d.as_millis() as i64);
                    out.push(StoredInfo {
                        hash,
                        size: meta.len(),
                        stored_at_ms,
                    });
                }
            }
        }
        Ok(out)
    }

    /// Removes the vault's whole folder.
    pub fn delete_vault(&self, vault: &VaultId) -> Result<(), FsBlobsError> {
        match fs::remove_dir_all(self.vault_root(vault)?) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(FsBlobsError::Io(e)),
            _ => Ok(()),
        }
    }

    pub fn verify(&self, vault: &VaultId, hash: &BlobHash) -> Result<VerifyResult, FsBlobsError> {
        let mut file = File::open(self.final_path(vault, hash)?).map_err(not_found_or_io)?;
        let (mut hasher, mut buf) = (Sha256::new(), vec![0u8; BUFFER]);
        loop {
            match file.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => hasher.update(&buf[..n]),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                Err(e) => return Err(FsBlobsError::Io(e)),
            }
        }
        let actual = hex_digest(hasher);
        Ok(if &actual == hash {
            VerifyResult::Ok
        } else {
            VerifyResult::Corrupt { actual }
        })
    }
}

fn not_found_or_io(e: io::Error) -> FsBlobsError {
    if e.kind() == io::ErrorKind::NotFound {
        FsBlobsError::NotFound
    } else {
        FsBlobsError::Io(e)
    }
}

fn sorted_names(dir: &Path) -> Result<Vec<String>, FsBlobsError> {
    let entries = match fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(FsBlobsError::Io(e)),
    };
    let mut names = vec![];
    for entry in entries {
        names.push(entry.map_err(FsBlobsError::Io)?.file_name().to_string_lossy().into_owned());
    }
    names.sort();
    Ok(names)
}
