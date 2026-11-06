//! Edge: content-addressed blob folders. Uploaded files live as plain files in `<root>/vaults/<vault>/blobs/ab/cd/<sha256>`.
// This crate's whole job is talking to the file system and reading its timestamps.
#![allow(clippy::disallowed_methods)]
mod fs_ops;
mod store;

pub use fs_ops::{FsOps, RealFs, Step};
pub use store::{FsBlobs, FsBlobsError, StoredInfo, VerifyResult};
