//! The folder store through the core trait: the same contract the fake passes, and the same reference flow.
// These tests create temporary folders.
#![allow(clippy::disallowed_methods)]
use crate::edges::FsBlobStore;
use memex_blob_fs::{FsBlobs, RealFs};
use memex_core::testing::{ManualClock, MemStore, blob_contract};
use memex_core::{BlobStore, Engine, NewSource, Snapshot, Store, Verified};
use memex_types::{BlobHash, ObjectId, Row, RowKey, Table, VaultId};

fn store(name: &str) -> (FsBlobStore<RealFs>, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("memex-edges-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    (FsBlobStore(FsBlobs::new(&dir, RealFs)), dir)
}

#[test]
fn folder_store_meets_the_blob_contract() {
    let (b, dir) = store("contract");
    blob_contract::run_all(&b, "c-");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_recorded_source_references_its_stored_file() {
    let (b, dir) = store("source");
    let engine = Engine::new(MemStore::new(), ManualClock::starting_at(0));
    let vault = VaultId("v".into());
    let lease = engine.store().open_vault(&vault).unwrap();
    let source = NewSource {
        id: ObjectId("s1".into()),
        actor: "a".into(),
        media_type: "application/pdf".into(),
        trust_tier: "unverified".into(),
        name: "a.pdf".into(),
    };
    engine.record_source(&b, &lease, source, &mut &b"pdf bytes"[..]).unwrap();
    let key = RowKey {
        table: Table::Source,
        id: ObjectId("s1".into()),
    };
    let Some(Row::Source(row)) = engine.store().snapshot(&vault).unwrap().get(&key).unwrap() else {
        panic!("source")
    };
    assert_eq!(b.verify(&vault, &BlobHash::parse(&row.sha256).unwrap()).unwrap(), Verified::Ok);
    std::fs::remove_dir_all(dir).unwrap();
}
