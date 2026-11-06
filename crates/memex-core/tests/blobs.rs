//! Blobs and their references, in memory.
use memex_core::testing::{BlobStep, ManualClock, MemBlobStore, MemStore, blob_contract};
use memex_core::{BlobStore, Clock, Engine, NewSource, RecordSourceError, Snapshot, Store, StoreError, Verified, dump_store, sweepable};
use memex_types::{BlobHash, ObjectId, Row, RowKey, Table, VaultId};
use std::collections::HashSet;
use std::time::Duration;

fn vault() -> VaultId {
    VaultId("v".into())
}

fn source(id: &str) -> NewSource {
    NewSource {
        id: ObjectId(id.into()),
        actor: "alice".into(),
        media_type: "application/pdf".into(),
        trust_tier: "unverified".into(),
        name: "a.pdf".into(),
    }
}

fn key(id: &str) -> RowKey {
    RowKey {
        table: Table::Source,
        id: ObjectId(id.into()),
    }
}

#[test]
fn mem_blob_store_meets_the_contract() {
    blob_contract::run_all(&MemBlobStore::new(ManualClock::starting_at(0)), "c-");
}

#[test]
fn a_failure_at_any_step_leaves_no_reference_and_the_retry_succeeds() {
    for step in [BlobStep::Write, BlobStep::Fsync, BlobStep::Rename, BlobStep::DirFsync] {
        let blobs = MemBlobStore::new(ManualClock::starting_at(0));
        let engine = Engine::new(MemStore::new(), ManualClock::starting_at(0));
        let lease = engine.store().open_vault(&vault()).unwrap();
        blobs.fail_next_put_at(step);
        assert!(
            matches!(
                engine.record_source(&blobs, &lease, source("s1"), &mut &b"pdf"[..]),
                Err(RecordSourceError::Blob(_))
            ),
            "{step:?}"
        );
        assert_eq!(engine.store().snapshot(&vault()).unwrap().get(&key("s1")).unwrap(), None, "{step:?}");
        assert!(blobs.list(&vault(), None, 10).unwrap().is_empty(), "{step:?}");
        engine.record_source(&blobs, &lease, source("s1"), &mut &b"pdf"[..]).unwrap();
        assert!(engine.store().snapshot(&vault()).unwrap().get(&key("s1")).unwrap().is_some(), "{step:?}");
    }
}

#[test]
fn a_failed_reference_commit_leaves_an_orphan_that_the_sweep_can_take_and_no_dangling_reference() {
    let blobs = MemBlobStore::new(ManualClock::starting_at(0));
    let engine = Engine::new(MemStore::new(), ManualClock::starting_at(0));
    let lease = engine.store().open_vault(&vault()).unwrap();
    engine.store().fail_next_commit(StoreError::Backend("connection lost".into()));
    assert!(matches!(
        engine.record_source(&blobs, &lease, source("s1"), &mut &b"pdf"[..]),
        Err(RecordSourceError::Apply(_))
    ));
    let snap = engine.store().snapshot(&vault()).unwrap();
    assert_eq!(snap.get(&key("s1")).unwrap(), None);
    drop(snap);
    let orphans = blobs.list(&vault(), None, 10).unwrap();
    assert_eq!(orphans.len(), 1);
    // The retry dedups onto the orphan and references it.
    engine.record_source(&blobs, &lease, source("s1"), &mut &b"pdf"[..]).unwrap();
    assert_eq!(blobs.list(&vault(), None, 10).unwrap().len(), 1);
}

#[test]
fn every_source_row_references_a_stored_blob_after_random_failures() {
    let blobs = MemBlobStore::new(ManualClock::starting_at(0));
    let engine = Engine::new(MemStore::new(), ManualClock::starting_at(0));
    let lease = engine.store().open_vault(&vault()).unwrap();
    let steps = [BlobStep::Write, BlobStep::Fsync, BlobStep::Rename, BlobStep::DirFsync];
    for i in 0u32..400 {
        match i % 5 {
            0..=3 => blobs.fail_next_put_at(steps[(i % 4) as usize]),
            _ => engine.store().fail_next_commit(StoreError::Backend("x".into())),
        }
        let bytes = i.to_le_bytes();
        let _ = engine.record_source(&blobs, &lease, source(&format!("s{i}")), &mut &bytes[..]);
        engine.record_source(&blobs, &lease, source(&format!("t{i}")), &mut &bytes[..]).unwrap();
    }
    let snap = engine.store().snapshot(&vault()).unwrap();
    let rows = snap.rows(Table::Source, None).unwrap();
    assert!(!rows.is_empty());
    for row in rows {
        let Row::Source(s) = row else { panic!("source") };
        let hash = BlobHash::parse(&s.sha256).unwrap();
        assert_eq!(blobs.verify(&vault(), &hash).unwrap(), Verified::Ok);
    }
    assert!(dump_store(&snap).is_ok());
}

#[test]
fn verify_reports_a_corrupted_blob() {
    let blobs = MemBlobStore::new(ManualClock::starting_at(0));
    let h = blobs.put(&vault(), &mut &b"bytes"[..]).unwrap().hash;
    blobs.corrupt(&vault(), &h);
    assert!(matches!(blobs.verify(&vault(), &h).unwrap(), Verified::Corrupt { actual } if actual != h));
}

#[test]
fn sweep_decisions_follow_the_clock_with_no_sleeping() {
    let blobs = MemBlobStore::new(ManualClock::starting_at(1000));
    let referenced = blobs.put(&vault(), &mut &b"kept"[..]).unwrap().hash;
    let orphan = blobs.put(&vault(), &mut &b"orphan"[..]).unwrap().hash;
    let refs = HashSet::from([referenced.clone()]);
    let grace = 60_000;
    let decide = |h: &BlobHash, now: i64| {
        let info = blobs.list(&vault(), None, 10).unwrap().into_iter().find(|i| &i.hash == h).unwrap();
        sweepable(&info, &refs, now, grace)
    };
    assert!(!decide(&orphan, blobs.clock().now_ms()));
    blobs.clock().advance(Duration::from_millis(60_000));
    assert!(decide(&orphan, blobs.clock().now_ms()));
    assert!(!decide(&referenced, blobs.clock().now_ms()));
}
