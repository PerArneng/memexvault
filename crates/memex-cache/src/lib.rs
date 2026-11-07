//! Edge: RAM and disk caches, each held to a byte cap. Keys come from the core's pure functions, so nothing here knows what it holds.
mod disk;
mod ram;

pub use disk::DiskTier;
pub use ram::RamTier;
