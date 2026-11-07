//! The cache tiers behind the core trait. It only translates, apart from weighing what goes in.
use memex_cache::{DiskTier, RamTier};
use memex_core::{CacheSettings, Caches};
use memex_types::{CacheTierStats, EvidencePackage, Row, VaultId};
use std::path::PathBuf;

/// A package or row is a tree of strings, so walking it would have to follow every new field. Its debug text has every string and a field name where a struct has its pointers and lengths, which comes out near its real size. `tests/cache_tiers.rs` reports the RSS this leads to.
fn weight_of(value: &impl std::fmt::Debug) -> u64 {
    use std::fmt::Write;
    /// Counts instead of allocating, as a package can be large.
    struct Counter(u64);
    impl Write for Counter {
        fn write_str(&mut self, s: &str) -> std::fmt::Result {
            self.0 += s.len() as u64;
            Ok(())
        }
    }
    let mut c = Counter(0);
    let _ = write!(c, "{value:?}");
    c.0
}

pub struct TieredCaches {
    query: RamTier<EvidencePackage>,
    inference: RamTier<Vec<f32>>,
    objects: RamTier<Row>,
    disk: Option<DiskTier>,
}

/// Everything off, for commands that don't serve.
impl Default for TieredCaches {
    fn default() -> Self {
        Self {
            query: RamTier::new("query", 0),
            inference: RamTier::new("inference", 0),
            objects: RamTier::new("objects", 0),
            disk: None,
        }
    }
}

impl TieredCaches {
    /// `disk_root` is the folder holding one folder per vault.
    pub fn open(settings: CacheSettings, disk_root: impl Into<PathBuf>) -> Result<Self, String> {
        Ok(Self {
            query: RamTier::new("query", settings.query_bytes),
            inference: RamTier::new("inference", settings.inference_bytes),
            objects: RamTier::new("objects", settings.object_bytes),
            disk: Some(DiskTier::open("disk", disk_root, settings.disk_bytes)?),
        })
    }

    fn disk(&self) -> Option<&DiskTier> {
        self.disk.as_ref().filter(|d| d.is_on())
    }
}

impl Caches for TieredCaches {
    fn any_ram_tier(&self) -> bool {
        self.query.is_on() || self.inference.is_on() || self.objects.is_on()
    }

    fn query_on(&self) -> bool {
        self.query.is_on()
    }

    fn inference_on(&self) -> bool {
        self.inference.is_on()
    }

    fn objects_on(&self) -> bool {
        self.objects.is_on()
    }

    fn disk_on(&self) -> bool {
        self.disk().is_some()
    }

    fn get_package(&self, key: &str) -> Option<EvidencePackage> {
        self.query.get(key).map(|p| (*p).clone())
    }

    fn put_package(&self, key: String, value: &EvidencePackage) {
        self.query.insert(key, value.clone(), weight_of(value));
    }

    fn get_floats(&self, key: &str) -> Option<Vec<f32>> {
        self.inference.get(key).map(|v| (*v).clone())
    }

    fn put_floats(&self, key: String, value: &[f32]) {
        let bytes = std::mem::size_of_val(value) as u64;
        self.inference.insert(key, value.to_vec(), bytes);
    }

    fn get_row(&self, key: &str) -> Option<Row> {
        self.objects.get(key).map(|r| (*r).clone())
    }

    fn put_row(&self, key: String, value: &Row) {
        self.objects.insert(key, value.clone(), weight_of(value));
    }

    fn get_file(&self, vault: &VaultId, key: &str) -> Option<Vec<u8>> {
        self.disk()?.get(&vault.0, key)
    }

    fn put_file(&self, vault: &VaultId, key: &str, value: &[u8]) {
        if let Some(d) = self.disk() {
            d.put(&vault.0, key, value);
        }
    }

    fn purge_vault(&self, vault: &VaultId) {
        let prefix = memex_core::cache::vault_prefix(vault);
        self.query.purge(&prefix);
        self.inference.purge(&prefix);
        self.objects.purge(&prefix);
        if let Some(d) = self.disk() {
            d.purge_vault(&vault.0);
        }
    }

    fn stats(&self) -> Vec<CacheTierStats> {
        let mut out = vec![self.query.stats(), self.inference.stats(), self.objects.stats()];
        if let Some(d) = &self.disk {
            out.push(d.stats());
        }
        out
    }
}
