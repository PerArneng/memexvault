//! The contract any BlobStore must meet.
use super::mem_blobs::sha256;
use crate::{BlobStore, BlobStoreError, Verified};
use memex_types::{BlobHash, VaultId};
use std::io::Read;

fn read_all<B: BlobStore>(b: &B, v: &VaultId, h: &BlobHash, offset: u64, len: Option<u64>) -> Vec<u8> {
    let mut out = vec![];
    b.read(v, h, offset, len).expect("read").read_to_end(&mut out).expect("read bytes");
    out
}

struct Broken;

impl Read for Broken {
    fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other("client went away"))
    }
}

/// Runs every check on fresh vault names under `prefix`.
pub fn run_all<B: BlobStore>(b: &B, prefix: &str) {
    let v = |n: &str| VaultId(format!("{prefix}{n}"));
    round_trip_and_hash(b, &v("round"));
    identical_bytes_are_a_no_op(b, &v("dedup"));
    ranges(b, &v("range"));
    empty_blob(b, &v("empty"));
    missing_blob_is_reported(b, &v("missing"));
    failed_input_stores_nothing(b, &v("input"));
    listing_pages_in_hash_order(b, &v("list"));
    delete_removes(b, &v("delete"));
    vaults_are_isolated(b, &v("iso-a"), &v("iso-b"));
}

fn round_trip_and_hash<B: BlobStore>(b: &B, v: &VaultId) {
    let data = b"Original document \xc3\xa5\xc3\xa4\xc3\xb6".to_vec();
    let stored = b.put(v, &mut data.as_slice()).expect("put");
    assert_eq!(stored.hash, sha256(&data));
    assert_eq!(stored.size, data.len() as u64);
    assert!(stored.newly_stored);
    assert_eq!(read_all(b, v, &stored.hash, 0, None), data);
    assert_eq!(b.verify(v, &stored.hash).expect("verify"), Verified::Ok);
}

fn identical_bytes_are_a_no_op<B: BlobStore>(b: &B, v: &VaultId) {
    let first = b.put(v, &mut &b"same"[..]).expect("put");
    let second = b.put(v, &mut &b"same"[..]).expect("put");
    assert_eq!(first.hash, second.hash);
    assert!(!second.newly_stored);
    assert_eq!(b.list(v, None, 10).expect("list").len(), 1);
}

fn ranges<B: BlobStore>(b: &B, v: &VaultId) {
    let h = b.put(v, &mut &b"0123456789"[..]).expect("put").hash;
    assert_eq!(read_all(b, v, &h, 2, Some(3)), b"234");
    assert_eq!(read_all(b, v, &h, 7, None), b"789");
    assert_eq!(read_all(b, v, &h, 8, Some(100)), b"89");
    assert_eq!(read_all(b, v, &h, 50, Some(5)), b"");
    assert_eq!(read_all(b, v, &h, 0, Some(0)), b"");
}

fn empty_blob<B: BlobStore>(b: &B, v: &VaultId) {
    let s = b.put(v, &mut &b""[..]).expect("put");
    assert_eq!(s.size, 0);
    assert_eq!(s.hash.as_str(), "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855");
    assert_eq!(read_all(b, v, &s.hash, 0, None), b"");
}

fn missing_blob_is_reported<B: BlobStore>(b: &B, v: &VaultId) {
    let ghost = sha256(b"never stored");
    assert!(matches!(b.read(v, &ghost, 0, None).err(), Some(BlobStoreError::NotFound { .. })));
    assert!(matches!(b.verify(v, &ghost), Err(BlobStoreError::NotFound { .. })));
    assert!(matches!(b.delete(v, &ghost), Err(BlobStoreError::NotFound { .. })));
    assert!(b.list(v, None, 10).expect("list").is_empty());
}

fn failed_input_stores_nothing<B: BlobStore>(b: &B, v: &VaultId) {
    assert!(matches!(b.put(v, &mut Broken), Err(BlobStoreError::Input(_))));
    assert!(b.list(v, None, 10).expect("list").is_empty());
}

fn listing_pages_in_hash_order<B: BlobStore>(b: &B, v: &VaultId) {
    for i in 0..5u8 {
        b.put(v, &mut &[i][..]).expect("put");
    }
    let first = b.list(v, None, 3).expect("list");
    assert_eq!(first.len(), 3);
    let rest = b.list(v, Some(&first[2].hash), 3).expect("list");
    assert_eq!(rest.len(), 2);
    let all: Vec<_> = first.iter().chain(&rest).map(|i| i.hash.clone()).collect();
    let mut sorted = all.clone();
    sorted.sort();
    assert_eq!(all, sorted);
    assert!(first.iter().all(|i| i.size == 1));
}

fn delete_removes<B: BlobStore>(b: &B, v: &VaultId) {
    let h = b.put(v, &mut &b"bye"[..]).expect("put").hash;
    b.delete(v, &h).expect("delete");
    assert!(matches!(b.read(v, &h, 0, None).err(), Some(BlobStoreError::NotFound { .. })));
    assert!(b.put(v, &mut &b"bye"[..]).expect("again").newly_stored);
}

fn vaults_are_isolated<B: BlobStore>(b: &B, a: &VaultId, other: &VaultId) {
    let h = b.put(a, &mut &b"secret"[..]).expect("put").hash;
    assert!(matches!(b.read(other, &h, 0, None).err(), Some(BlobStoreError::NotFound { .. })));
    assert!(b.list(other, None, 10).expect("list").is_empty());
}
