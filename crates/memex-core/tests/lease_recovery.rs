//! Spec 089: a server that loses a vault's writer lease takes it again, with backoff, and never while another writer holds it. In memory.
use memex_core::testing::{ManualClock, MemBlobStore, MemRegistry, MemStore, SeededRandom};
use memex_core::{Caller, LEASE_REGAINING, REGAIN_MAX_MS, Server, ServerEdges, ServerError, Store, StoreError, UploadLimits, UploadRequest, regain_delay_ms};
use memex_types::VaultId;
use serde_json::json;
use std::sync::atomic::Ordering;
use std::time::Duration;

struct Fakes;

impl ServerEdges for Fakes {
    type Registry = MemRegistry;
    type Store = MemStore;
    type Blobs = MemBlobStore<ManualClock>;
    type Clock = ManualClock;
    type Seed = SeededRandom;
    type Rasterizer = memex_core::testing::FakeRasterizer;
    type Lexical = memex_core::testing::MemLexical;
    type Vector = memex_core::testing::MemVector;
    type Embedder = memex_core::testing::FakeEmbedder;
    type Reranker = memex_core::testing::FakeReranker;
    type Caches = memex_core::NoCaches;
}

struct World {
    s: Server<Fakes>,
    admin: Caller,
    v: VaultId,
}

fn world() -> World {
    let s = Server::<Fakes>::new(
        MemRegistry::new(),
        MemStore::new(),
        MemBlobStore::new(ManualClock::starting_at(0)),
        ManualClock::starting_at(1_000_000),
        SeededRandom::new(89),
        memex_core::testing::mem_search(),
    );
    let admin = s.authenticate(&s.bootstrap("root").unwrap().token).unwrap();
    let v = s.create_vault(&admin, "v").unwrap().id;
    World { s, admin, v }
}

const LIMITS: UploadLimits = UploadLimits {
    max_bytes: 1000,
    max_open_jobs: 10_000,
};

fn upload(w: &World, n: u32) -> Result<memex_core::UploadOutcome, ServerError> {
    let req = UploadRequest {
        name: format!("d{n}.txt"),
        trust_tier: "unverified".into(),
        metadata: json!({}),
        replaces: None,
    };
    let bytes = format!("document number {n}");
    w.s.upload(&w.admin, &w.v, req, LIMITS, &mut bytes.as_bytes())
}

fn store(w: &World) -> &MemStore {
    w.s.engine().store()
}

fn opens(w: &World) -> u64 {
    store(w).counts().opens.load(Ordering::Relaxed)
}

fn advance(w: &World, ms: i64) {
    w.s.engine().clock().advance(Duration::from_millis(ms as u64));
}

fn sources(w: &World) -> usize {
    w.s.list_sources(&w.admin, &w.v, None, 200).unwrap().len()
}

fn away() -> StoreError {
    StoreError::Backend("connection refused".into())
}

#[test]
fn the_delay_doubles_from_the_first_to_the_most() {
    let delays: Vec<i64> = (1..=8).map(regain_delay_ms).collect();
    assert_eq!(delays, vec![250, 500, 1000, 2000, 4000, 5000, 5000, 5000]);
    assert_eq!(regain_delay_ms(u32::MAX), REGAIN_MAX_MS);
}

#[test]
fn a_lost_lease_is_taken_again_and_the_next_write_works() {
    let w = world();
    upload(&w, 0).unwrap();
    let before = opens(&w);
    store(&w).lose_lease(&w.v);
    assert!(w.s.lease_lost(&w.v));
    upload(&w, 1).unwrap();
    assert_eq!(opens(&w), before + 1, "one open regains it");
    assert!(!w.s.lease_lost(&w.v));
    // A held lease isn't opened again.
    upload(&w, 2).unwrap();
    assert_eq!(opens(&w), before + 1);
    assert_eq!(sources(&w), 3);
}

#[test]
fn a_claim_regains_the_lease_too() {
    let w = world();
    upload(&w, 0).unwrap();
    store(&w).lose_lease(&w.v);
    let job = w.s.claim_job(&w.v, memex_core::RENDITION_JOB, "worker").unwrap().expect("the upload's job");
    assert!(w.s.complete_job(&w.v, &job, vec![]).unwrap());
    assert!(!w.s.lease_lost(&w.v));
}

/// A minute away, with a write tried every 100 ms: each fails with 503, the store is asked at most once per delay, and the first write after it's back works within the longest delay.
#[test]
fn during_an_outage_writes_fail_and_tries_back_off() {
    let w = world();
    for n in 0..5 {
        upload(&w, n).unwrap();
    }
    store(&w).lose_lease(&w.v);
    store(&w).fail_next_opens(u32::MAX, away());
    let before = opens(&w);
    let mut refused = 0;
    let mut regaining = 0;
    for step in 0..600 {
        match upload(&w, 100 + step) {
            Err(ServerError::Unavailable(m)) => {
                refused += 1;
                if m == LEASE_REGAINING {
                    regaining += 1;
                }
            }
            other => panic!("a write during the outage got {other:?}"),
        }
        assert!(w.s.lease_lost(&w.v));
        advance(&w, 100);
    }
    let tries = opens(&w) - before;
    // 0, 250, 750, 1750, 3750, 7750 and then every 5 s up to 60 s: 17 tries.
    assert!((15..=17).contains(&tries), "{tries} tries in 60 s");
    assert_eq!(refused, 600);
    assert_eq!(regaining as u64, 600 - tries, "only the tries reached the store");
    store(&w).fail_next_opens(0, away());
    let mut waited = 0;
    while upload(&w, 1000).is_err() {
        advance(&w, 100);
        waited += 100;
        assert!(waited <= REGAIN_MAX_MS, "still refused {waited} ms after the store came back");
    }
    assert!(!w.s.lease_lost(&w.v));
    assert_eq!(sources(&w), 6, "the five acknowledged before and the one after, and nothing from the outage");
}

#[test]
fn another_writer_that_took_the_lease_keeps_it() {
    let w = world();
    upload(&w, 0).unwrap();
    store(&w).lose_lease(&w.v);
    let other = store(&w).open_vault(&w.v).unwrap();
    for step in 0..100 {
        assert!(matches!(upload(&w, 10 + step), Err(ServerError::Unavailable(_))));
        assert!(store(&w).lease_held(&other), "the server took the lease from the writer holding it");
        advance(&w, 1000);
    }
    // The other writer still writes, and the server's attempts changed nothing.
    assert!(store(&w).lease_held(&other));
    store(&w).close_vault(&other).unwrap();
    advance(&w, REGAIN_MAX_MS);
    upload(&w, 1).unwrap();
    assert_eq!(sources(&w), 2);
}

#[test]
fn deleting_a_vault_whose_lease_was_lost_works() {
    let w = world();
    upload(&w, 0).unwrap();
    store(&w).lose_lease(&w.v);
    w.s.delete_vault(&w.admin, &w.v).unwrap();
    assert!(!w.s.lease_lost(&w.v));
}
