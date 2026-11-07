//! An LRU cache of files, for page images and word tables. It survives a restart, and anything it can't trust is a miss.
// Files are this module's job, and recency on disk is the file's modified time.
#![allow(clippy::disallowed_methods)]
use memex_types::CacheTierStats;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File, FileTimes};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

const MAGIC: &[u8; 4] = b"MXC1";
const CACHE_DIR: &str = "cache";
const TMP_SUFFIX: &str = ".tmp";
const DIGEST_BYTES: usize = 32;

#[derive(Debug, Clone)]
struct Held {
    vault: String,
    size: u64,
    tick: u64,
}

#[derive(Default)]
struct Index {
    files: HashMap<PathBuf, Held>,
    by_age: BTreeMap<u64, PathBuf>,
    next_tick: u64,
    size: u64,
}

impl Index {
    fn add(&mut self, path: PathBuf, vault: String, size: u64) {
        self.drop_entry(&path);
        let tick = self.next_tick;
        self.next_tick += 1;
        self.by_age.insert(tick, path.clone());
        self.files.insert(path, Held { vault, size, tick });
        self.size += size;
    }

    fn drop_entry(&mut self, path: &Path) -> bool {
        match self.files.remove(path) {
            Some(h) => {
                self.by_age.remove(&h.tick);
                self.size -= h.size;
                true
            }
            None => false,
        }
    }

    fn touch(&mut self, path: &Path) {
        if let Some(h) = self.files.get_mut(path) {
            self.by_age.remove(&h.tick);
            h.tick = self.next_tick;
            self.next_tick += 1;
            self.by_age.insert(h.tick, path.to_path_buf());
        }
    }
}

/// Files live at `<root>/<vault>/cache/ab/<sha256 of key>`, so removing a vault's folder removes them. One cap covers every vault.
pub struct DiskTier {
    name: &'static str,
    root: PathBuf,
    cap: u64,
    index: Mutex<Index>,
    tmp_counter: AtomicU64,
    hits: AtomicU64,
    misses: AtomicU64,
    evictions: AtomicU64,
    rejected: AtomicU64,
    corrupt: AtomicU64,
}

fn digest_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// A vault's name becomes a folder name, so anything that could leave the root is refused.
fn safe_vault(vault: &str) -> bool {
    !vault.is_empty() && vault != "." && vault != ".." && !vault.contains(['/', '\\', '\0'])
}

fn encode(key: &str, value: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(MAGIC.len() + 4 + key.len() + value.len() + DIGEST_BYTES);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&(key.len() as u32).to_le_bytes());
    out.extend_from_slice(key.as_bytes());
    out.extend_from_slice(value);
    let digest = Sha256::digest(&out);
    out.extend_from_slice(&digest);
    out
}

/// The value, if the file is whole and is for this key. A key in the file means no name clash or copied file can answer for another key.
fn decode<'a>(bytes: &'a [u8], key: &str) -> Option<&'a [u8]> {
    let body_end = bytes.len().checked_sub(DIGEST_BYTES)?;
    let (body, digest) = bytes.split_at(body_end);
    if Sha256::digest(body).as_slice() != digest || !body.starts_with(MAGIC) {
        return None;
    }
    let rest = &body[MAGIC.len()..];
    let (len, rest) = rest.split_at_checked(4)?;
    let len = u32::from_le_bytes(len.try_into().ok()?) as usize;
    let (stored, value) = rest.split_at_checked(len)?;
    (stored == key.as_bytes()).then_some(value)
}

impl DiskTier {
    /// Finds what an earlier run left, oldest first, and leaves it within the cap. A cap of 0 turns the tier off and leaves the disk alone.
    pub fn open(name: &'static str, root: impl Into<PathBuf>, cap: u64) -> Result<Self, String> {
        let tier = Self {
            name,
            root: root.into(),
            cap,
            index: Mutex::new(Index::default()),
            tmp_counter: AtomicU64::new(0),
            hits: AtomicU64::new(0),
            misses: AtomicU64::new(0),
            evictions: AtomicU64::new(0),
            rejected: AtomicU64::new(0),
            corrupt: AtomicU64::new(0),
        };
        if cap > 0 {
            tier.scan().map_err(|e| format!("can't read the disk cache at {}: {e}", tier.root.display()))?;
            let mut index = tier.index.lock().expect("lock");
            tier.make_room(&mut index, 0);
        }
        Ok(tier)
    }

    fn scan(&self) -> std::io::Result<()> {
        let mut found: Vec<(SystemTime, PathBuf, String, u64)> = vec![];
        let vaults = match fs::read_dir(&self.root) {
            Ok(v) => v,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        };
        for vault in vaults {
            let vault = vault?;
            let Some(name) = vault.file_name().to_str().map(String::from) else { continue };
            let Ok(shards) = fs::read_dir(vault.path().join(CACHE_DIR)) else { continue };
            for shard in shards {
                let Ok(files) = fs::read_dir(shard?.path()) else { continue };
                for file in files {
                    let file = file?;
                    let path = file.path();
                    // A crash can leave half a write, which nothing refers to.
                    if path.to_string_lossy().ends_with(TMP_SUFFIX) {
                        let _ = fs::remove_file(&path);
                        continue;
                    }
                    let meta = file.metadata()?;
                    found.push((meta.modified().unwrap_or(SystemTime::UNIX_EPOCH), path, name.clone(), meta.len()));
                }
            }
        }
        found.sort_by_key(|f| f.0);
        let mut index = self.index.lock().expect("lock");
        for (_, path, vault, size) in found {
            index.add(path, vault, size);
        }
        Ok(())
    }

    pub fn is_on(&self) -> bool {
        self.cap > 0
    }

    fn path_of(&self, vault: &str, key: &str) -> PathBuf {
        let name = digest_hex(key.as_bytes());
        self.root.join(vault).join(CACHE_DIR).join(&name[..2]).join(name)
    }

    /// Removes the oldest until `incoming` more bytes fit, so the tier is at or under its cap at every moment.
    fn make_room(&self, index: &mut Index, incoming: u64) {
        while index.size + incoming > self.cap {
            let Some(path) = index.by_age.first_key_value().map(|(_, p)| p.clone()) else {
                return;
            };
            index.drop_entry(&path);
            let _ = fs::remove_file(&path);
            self.evictions.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn get(&self, vault: &str, key: &str) -> Option<Vec<u8>> {
        if !self.is_on() || !safe_vault(vault) {
            return None;
        }
        let path = self.path_of(vault, key);
        let Ok(bytes) = fs::read(&path) else {
            // Gone behind our back, for instance with its vault.
            self.index.lock().expect("lock").drop_entry(&path);
            self.misses.fetch_add(1, Ordering::Relaxed);
            return None;
        };
        match decode(&bytes, key) {
            Some(value) => {
                self.index.lock().expect("lock").touch(&path);
                // The next run's only record of recency. Failing to set it costs ordering, not correctness.
                if let Ok(f) = File::options().write(true).open(&path) {
                    let _ = f.set_times(FileTimes::new().set_modified(SystemTime::now()));
                }
                self.hits.fetch_add(1, Ordering::Relaxed);
                Some(value.to_vec())
            }
            None => {
                self.index.lock().expect("lock").drop_entry(&path);
                let _ = fs::remove_file(&path);
                self.corrupt.fetch_add(1, Ordering::Relaxed);
                self.misses.fetch_add(1, Ordering::Relaxed);
                None
            }
        }
    }

    /// Best effort: a cache that can't write just misses next time. Not synced, since a torn file is a miss.
    pub fn put(&self, vault: &str, key: &str, value: &[u8]) {
        if !self.is_on() || !safe_vault(vault) {
            return;
        }
        let bytes = encode(key, value);
        let size = bytes.len() as u64;
        if size > self.cap {
            self.rejected.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let path = self.path_of(vault, key);
        {
            // The room is claimed before the write, so concurrent writes can't pass the cap together.
            let mut index = self.index.lock().expect("lock");
            index.drop_entry(&path);
            self.make_room(&mut index, size);
            index.add(path.clone(), vault.to_string(), size);
        }
        if let Err(e) = self.write_file(&path, &bytes) {
            tracing::warn!(error = %e, "can't write to the disk cache");
            self.index.lock().expect("lock").drop_entry(&path);
            let _ = fs::remove_file(&path);
        }
    }

    fn write_file(&self, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
        let dir = path.parent().ok_or_else(|| std::io::Error::other("a cache file has no folder"))?;
        fs::create_dir_all(dir)?;
        let n = self.tmp_counter.fetch_add(1, Ordering::Relaxed);
        let tmp = path.with_extension(format!("{n}{TMP_SUFFIX}"));
        let result = File::create(&tmp).and_then(|mut f| f.write_all(bytes)).and_then(|()| fs::rename(&tmp, path));
        if result.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        result
    }

    /// Removes all of a vault's files, for erasure and for deleting the vault.
    pub fn purge_vault(&self, vault: &str) {
        if !self.is_on() || !safe_vault(vault) {
            return;
        }
        let mut index = self.index.lock().expect("lock");
        let paths: Vec<PathBuf> = index.files.iter().filter(|(_, h)| h.vault == vault).map(|(p, _)| p.clone()).collect();
        for p in paths {
            index.drop_entry(&p);
        }
        let _ = fs::remove_dir_all(self.root.join(vault).join(CACHE_DIR));
    }

    pub fn stats(&self) -> CacheTierStats {
        let index = self.index.lock().expect("lock");
        CacheTierStats {
            tier: self.name.to_string(),
            cap_bytes: self.cap,
            size_bytes: index.size,
            entries: index.files.len() as u64,
            hits: self.hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            evictions: self.evictions.load(Ordering::Relaxed),
            rejected: self.rejected.load(Ordering::Relaxed),
            corrupt: self.corrupt.load(Ordering::Relaxed),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh folder under the build's temp dir, gone when dropped.
    struct Dir(PathBuf);

    impl Dir {
        fn new(name: &str) -> Self {
            let d = std::env::temp_dir().join(format!("memex-disk-tier-{}-{name}", std::process::id()));
            let _ = fs::remove_dir_all(&d);
            fs::create_dir_all(&d).unwrap();
            Self(d)
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn cache_files(root: &Path) -> Vec<PathBuf> {
        let mut out = vec![];
        let mut stack = vec![root.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in fs::read_dir(d).unwrap() {
                let p = e.unwrap().path();
                if p.is_dir() {
                    stack.push(p);
                } else {
                    out.push(p);
                }
            }
        }
        out
    }

    fn on_disk(root: &Path) -> u64 {
        cache_files(root).iter().map(|p| fs::metadata(p).unwrap().len()).sum()
    }

    #[test]
    fn a_value_comes_back_and_a_missing_one_is_a_miss() {
        let d = Dir::new("basic");
        let t = DiskTier::open("disk", &d.0, 1 << 20).unwrap();
        assert_eq!(t.get("v", "k"), None);
        t.put("v", "k", b"hello");
        assert_eq!(t.get("v", "k").as_deref(), Some(&b"hello"[..]));
        let s = t.stats();
        assert_eq!((s.hits, s.misses, s.entries), (1, 1, 1));
        assert_eq!(s.size_bytes, on_disk(&d.0));
    }

    #[test]
    fn it_survives_a_restart() {
        let d = Dir::new("restart");
        DiskTier::open("disk", &d.0, 1 << 20).unwrap().put("v", "k", b"kept");
        let again = DiskTier::open("disk", &d.0, 1 << 20).unwrap();
        assert_eq!(again.get("v", "k").as_deref(), Some(&b"kept"[..]));
        assert_eq!(again.stats().entries, 1);
    }

    #[test]
    fn a_corrupt_file_is_a_miss_and_is_removed() {
        for damage in ["flip", "truncate", "empty", "garbage"] {
            let d = Dir::new(&format!("corrupt-{damage}"));
            let t = DiskTier::open("disk", &d.0, 1 << 20).unwrap();
            t.put("v", "k", b"some value that matters");
            let path = t.path_of("v", "k");
            let mut bytes = fs::read(&path).unwrap();
            match damage {
                "flip" => {
                    let at = bytes.len() / 2;
                    bytes[at] ^= 0xff;
                }
                "truncate" => bytes.truncate(bytes.len() / 2),
                "empty" => bytes.clear(),
                _ => bytes = b"not a cache file at all, and long enough to hold a digest".to_vec(),
            }
            fs::write(&path, bytes).unwrap();
            assert_eq!(t.get("v", "k"), None, "{damage}");
            assert!(!path.exists(), "{damage}");
            let s = t.stats();
            assert_eq!((s.corrupt, s.entries, s.size_bytes), (1, 0, 0), "{damage}");
            t.put("v", "k", b"again");
            assert_eq!(t.get("v", "k").as_deref(), Some(&b"again"[..]), "{damage}");
        }
    }

    #[test]
    fn a_corrupt_file_found_at_startup_is_still_a_miss() {
        let d = Dir::new("corrupt-restart");
        let t = DiskTier::open("disk", &d.0, 1 << 20).unwrap();
        t.put("v", "k", b"value");
        fs::write(t.path_of("v", "k"), b"junk").unwrap();
        let again = DiskTier::open("disk", &d.0, 1 << 20).unwrap();
        assert_eq!(again.get("v", "k"), None);
        assert_eq!(again.stats().corrupt, 1);
    }

    #[test]
    fn a_file_copied_to_another_key_or_vault_does_not_answer_for_it() {
        let d = Dir::new("copied");
        let t = DiskTier::open("disk", &d.0, 1 << 20).unwrap();
        t.put("a", "k-a", b"vault a's data");
        t.put("b", "k-b", b"vault b's data");
        // The worst case: another vault's file lands exactly where this key's should be.
        fs::copy(t.path_of("a", "k-a"), t.path_of("b", "k-b")).unwrap();
        assert_eq!(t.get("b", "k-b"), None);
        assert_eq!(t.stats().corrupt, 1);
    }

    #[test]
    fn the_same_key_in_two_vaults_stays_apart() {
        let d = Dir::new("vaults");
        let t = DiskTier::open("disk", &d.0, 1 << 20).unwrap();
        t.put("a", "k", b"a's");
        t.put("b", "k", b"b's");
        assert_eq!(t.get("a", "k").as_deref(), Some(&b"a's"[..]));
        assert_eq!(t.get("b", "k").as_deref(), Some(&b"b's"[..]));
        assert_eq!(t.get("c", "k"), None);
    }

    #[test]
    fn a_vault_name_cannot_leave_the_root() {
        let d = Dir::new("escape");
        let t = DiskTier::open("disk", &d.0, 1 << 20).unwrap();
        for bad in ["../evil", "..", ".", "", "a/b", "a\\b"] {
            t.put(bad, "k", b"x");
            assert_eq!(t.get(bad, "k"), None, "{bad}");
        }
        assert_eq!(t.stats().entries, 0);
    }

    #[test]
    fn eviction_keeps_it_within_the_cap_and_takes_the_oldest() {
        let d = Dir::new("evict");
        let cap = 50_000;
        let t = DiskTier::open("disk", &d.0, cap).unwrap();
        t.put("v", "first", &[0; 1000]);
        for i in 0..500 {
            t.put("v", &format!("k{i}"), &[0; 1000]);
            assert!(t.stats().size_bytes <= cap);
            assert!(on_disk(&d.0) <= cap);
            // Reading it keeps it, so the oldest is something else.
            if i % 5 == 0 {
                t.get("v", "k0");
            }
        }
        let s = t.stats();
        assert!(s.evictions > 400, "{s:?}");
        assert_eq!(t.get("v", "first"), None);
        assert!(t.get("v", "k0").is_some());
        assert!(t.get("v", "k499").is_some());
    }

    #[test]
    fn a_smaller_cap_at_startup_evicts_the_oldest_by_file_time() {
        let d = Dir::new("shrink");
        let t = DiskTier::open("disk", &d.0, 1 << 20).unwrap();
        for k in ["old", "mid", "new"] {
            t.put("v", k, &[0; 1000]);
        }
        let base = SystemTime::now() - std::time::Duration::from_secs(100);
        for (i, k) in ["old", "mid", "new"].iter().enumerate() {
            let f = File::options().write(true).open(t.path_of("v", k)).unwrap();
            f.set_times(FileTimes::new().set_modified(base + std::time::Duration::from_secs(i as u64)))
                .unwrap();
        }
        drop(t);
        let again = DiskTier::open("disk", &d.0, 2300).unwrap();
        assert!(again.stats().size_bytes <= 2300);
        assert_eq!(again.get("v", "old"), None);
        assert!(again.get("v", "new").is_some());
    }

    #[test]
    fn a_value_bigger_than_the_cap_is_refused() {
        let d = Dir::new("toobig");
        let t = DiskTier::open("disk", &d.0, 100).unwrap();
        t.put("v", "k", &[0; 1000]);
        assert_eq!(t.get("v", "k"), None);
        assert_eq!(t.stats().rejected, 1);
        assert_eq!(on_disk(&d.0), 0);
    }

    #[test]
    fn half_a_write_from_a_crash_is_swept_at_startup() {
        let d = Dir::new("crash");
        let t = DiskTier::open("disk", &d.0, 1 << 20).unwrap();
        t.put("v", "k", b"x");
        let tmp = t.path_of("v", "k").with_extension(format!("0{TMP_SUFFIX}"));
        fs::write(&tmp, b"half").unwrap();
        let again = DiskTier::open("disk", &d.0, 1 << 20).unwrap();
        assert!(!tmp.exists());
        assert_eq!(again.stats().entries, 1);
    }

    #[test]
    fn a_cap_of_zero_does_nothing() {
        let d = Dir::new("off");
        let t = DiskTier::open("disk", &d.0, 0).unwrap();
        assert!(!t.is_on());
        t.put("v", "k", b"x");
        assert_eq!(t.get("v", "k"), None);
        assert_eq!(cache_files(&d.0).len(), 0);
        let s = t.stats();
        assert_eq!((s.hits, s.misses), (0, 0));
    }

    #[test]
    fn purging_a_vault_removes_its_files_only() {
        let d = Dir::new("purge");
        let t = DiskTier::open("disk", &d.0, 1 << 20).unwrap();
        t.put("a", "k", b"a");
        t.put("b", "k", b"b");
        t.purge_vault("a");
        assert_eq!(t.get("a", "k"), None);
        assert!(!d.0.join("a").join(CACHE_DIR).exists());
        assert!(t.get("b", "k").is_some());
        assert_eq!(t.stats().entries, 1);
    }

    #[test]
    fn a_file_deleted_with_its_vault_is_a_miss_and_is_forgotten() {
        let d = Dir::new("gone");
        let t = DiskTier::open("disk", &d.0, 1 << 20).unwrap();
        t.put("a", "k", b"a");
        fs::remove_dir_all(d.0.join("a")).unwrap();
        assert_eq!(t.get("a", "k"), None);
        assert_eq!(t.stats().size_bytes, 0);
    }

    #[test]
    fn writers_at_once_stay_within_the_cap() {
        let d = Dir::new("threads");
        let cap = 100_000;
        let t = DiskTier::open("disk", &d.0, cap).unwrap();
        std::thread::scope(|s| {
            for w in 0..4 {
                let t = &t;
                s.spawn(move || {
                    for i in 0..500 {
                        t.put("v", &format!("{w}-{i}"), &[w as u8; 900]);
                        assert!(t.stats().size_bytes <= cap);
                    }
                });
            }
        });
        assert!(on_disk(&d.0) <= cap);
        assert_eq!(t.stats().size_bytes, on_disk(&d.0));
    }
}
