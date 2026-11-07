/// One cache tier's counters, since this server started.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CacheTierStats {
    pub tier: String,
    /// 0 means the tier is off.
    pub cap_bytes: u64,
    pub size_bytes: u64,
    pub entries: u64,
    pub hits: u64,
    pub misses: u64,
    pub evictions: u64,
    /// Entries too big for the tier to keep.
    pub rejected: u64,
    /// Files that couldn't be trusted, each counted as a miss too. Only the disk tier has these.
    pub corrupt: u64,
}
