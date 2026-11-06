//! The event log through the facade, entirely in memory.
use memex_core::testing::{ManualClock, MemStore, SeededRandom, contract, generate::random_draft};
use memex_core::{ApplyError, Engine, ReduceError, ReplayError, Snapshot, Store, StoreError, UpcastError, all_events, dump_store, replay, upcast};
use memex_types::{Event, EventDraft, ObjectId, Row, RowKey, Table, VaultId};
use serde_json::json;
use std::sync::atomic::Ordering;
use std::time::Duration;

fn vault() -> VaultId {
    VaultId("v".into())
}

fn engine() -> Engine<MemStore, ManualClock> {
    Engine::new(MemStore::new(), ManualClock::starting_at(1000))
}

fn create(id: &str) -> EventDraft {
    EventDraft {
        actor: "alice".into(),
        kind: "page.created".into(),
        schema_version: 1,
        payload: json!({"id": id, "title": "T", "body": "B"}),
    }
}

fn page_key(id: &str) -> RowKey {
    RowKey {
        table: Table::Page,
        id: ObjectId(id.into()),
    }
}

fn fixture(text: &str) -> Vec<Event> {
    text.lines().map(|l| serde_json::from_str(l).expect("fixture")).collect()
}

#[test]
fn mem_store_meets_the_store_contract() {
    contract::run_all(&MemStore::new(), "c-");
}

#[test]
fn replaying_100k_random_events_reproduces_identical_state() {
    let (applied, refused, source, target) = contract::random_replay(MemStore::new(), &MemStore::new(), &VaultId("random-source".into()), 42, 100_000);
    assert_eq!(source, target);
    // The generator must really exercise both paths.
    assert!(applied > 20_000 && refused > 20_000, "applied {applied}, refused {refused}");
}

#[test]
fn different_seeds_give_different_state() {
    let (_, _, a, _) = contract::random_replay(MemStore::new(), &MemStore::new(), &VaultId("random-source".into()), 1, 2000);
    let (_, _, b, _) = contract::random_replay(MemStore::new(), &MemStore::new(), &VaultId("random-source".into()), 2, 2000);
    assert_ne!(a, b);
}

#[test]
fn same_rows_in_any_insertion_order_have_the_same_hash() {
    let (one, two) = (MemStore::new(), MemStore::new());
    let (la, lb) = (one.open_vault(&vault()).unwrap(), two.open_vault(&vault()).unwrap());
    let (e1, e2) = (Engine::new(one, ManualClock::starting_at(5)), Engine::new(two, ManualClock::starting_at(5)));
    for id in ["a", "b", "c"] {
        e1.apply(&la, create(id)).unwrap();
    }
    for id in ["c", "a", "b"] {
        e2.apply(&lb, create(id)).unwrap();
    }
    let hash = |e: &Engine<MemStore, ManualClock>| dump_store(&e.store().snapshot(&vault()).unwrap()).unwrap().sha256;
    assert_eq!(hash(&e1), hash(&e2));
}

#[test]
fn applying_one_event_makes_exactly_one_commit() {
    let e = engine();
    let lease = e.store().open_vault(&vault()).unwrap();
    e.apply(&lease, create("p")).unwrap();
    assert_eq!(e.store().counts().commits.load(Ordering::Relaxed), 1);
    e.apply(&lease, create("q")).unwrap();
    assert_eq!(e.store().counts().commits.load(Ordering::Relaxed), 2);
}

#[test]
fn a_refused_event_makes_no_commit() {
    let e = engine();
    let lease = e.store().open_vault(&vault()).unwrap();
    e.apply(&lease, create("p")).unwrap();
    assert!(matches!(e.apply(&lease, create("p")), Err(ApplyError::Reduce(ReduceError::AlreadyExists(_)))));
    assert_eq!(e.store().counts().commits.load(Ordering::Relaxed), 1);
    assert_eq!(e.store().snapshot(&vault()).unwrap().head().unwrap(), Some(0));
}

#[test]
fn get_many_is_one_store_call() {
    let e = engine();
    let lease = e.store().open_vault(&vault()).unwrap();
    for i in 0..20 {
        e.apply(&lease, create(&format!("p{i}"))).unwrap();
    }
    let keys: Vec<RowKey> = (0..20).map(|i| page_key(&format!("p{i}"))).collect();
    let before = e.store().counts().reads.load(Ordering::Relaxed);
    let rows = e.store().snapshot(&vault()).unwrap().get_many(&keys).unwrap();
    assert_eq!(rows.iter().flatten().count(), 20);
    assert_eq!(e.store().counts().reads.load(Ordering::Relaxed) - before, 1);
}

#[test]
fn an_aborted_transaction_leaves_no_trace() {
    let e = engine();
    let lease = e.store().open_vault(&vault()).unwrap();
    e.apply(&lease, create("kept")).unwrap();
    let before = dump_store(&e.store().snapshot(&vault()).unwrap()).unwrap();
    e.store().fail_next_commit(StoreError::Backend("connection lost".into()));
    assert!(matches!(e.apply(&lease, create("lost")), Err(ApplyError::Store(StoreError::Backend(_)))));
    let snap = e.store().snapshot(&vault()).unwrap();
    assert_eq!(dump_store(&snap).unwrap(), before);
    assert_eq!(snap.get(&page_key("lost")).unwrap(), None);
    assert_eq!(all_events(&snap).unwrap().len(), 1);
    drop(snap);
    // The sequence isn't burned, so the log stays contiguous.
    assert_eq!(e.apply(&lease, create("next")).unwrap().seq, 1);
}

#[test]
fn a_batch_reduces_each_draft_on_the_ones_before_it_and_commits_once() {
    let e = engine();
    let lease = e.store().open_vault(&vault()).unwrap();
    let edit = EventDraft {
        actor: "b".into(),
        kind: "page.edited".into(),
        schema_version: 1,
        payload: json!({"id": "p", "base_version": 1, "body": "new"}),
    };
    let stale = EventDraft {
        actor: "b".into(),
        kind: "page.edited".into(),
        schema_version: 1,
        payload: json!({"id": "p", "base_version": 1, "body": "lost"}),
    };
    let results = e.apply_batch(&lease, vec![create("p"), edit, stale, create("q")]);
    assert_eq!(results[0].as_ref().unwrap().seq, 0);
    assert_eq!(results[1].as_ref().unwrap().seq, 1);
    assert!(matches!(
        results[2],
        Err(ApplyError::Reduce(ReduceError::StaleVersion { current: 2, base: 1, .. }))
    ));
    assert_eq!(results[3].as_ref().unwrap().seq, 2);
    assert_eq!(e.store().counts().commits.load(Ordering::Relaxed), 1);
    let snap = e.store().snapshot(&vault()).unwrap();
    assert_eq!(snap.head().unwrap(), Some(2));
    let Some(Row::Page(p)) = snap.get(&page_key("p")).unwrap() else {
        panic!("page")
    };
    assert_eq!((p.header.version, p.body.as_str()), (2, "new"));
}

#[test]
fn a_failed_batch_gives_every_accepted_draft_the_error_and_leaves_no_trace() {
    let e = engine();
    let lease = e.store().open_vault(&vault()).unwrap();
    e.store().fail_next_commit(StoreError::Backend("connection lost".into()));
    let results = e.apply_batch(&lease, vec![create("p"), create("q")]);
    assert!(results.iter().all(|r| matches!(r, Err(ApplyError::Store(StoreError::Backend(_))))));
    assert_eq!(e.store().snapshot(&vault()).unwrap().head().unwrap(), None);
    assert_eq!(e.apply_batch(&lease, vec![create("p")])[0].as_ref().unwrap().seq, 0);
}

#[test]
fn a_batch_gives_the_same_state_as_applying_one_by_one() {
    let (one, many) = (engine(), engine());
    let (l1, l2) = (one.store().open_vault(&vault()).unwrap(), many.store().open_vault(&vault()).unwrap());
    let (r1, r2) = (SeededRandom::new(11), SeededRandom::new(11));
    for _ in 0..200 {
        let drafts: Vec<EventDraft> = (0..13).map(|_| random_draft(&r1, 30)).collect();
        for r in many.apply_batch(&l2, drafts) {
            assert!(!matches!(r, Err(ApplyError::Store(_))));
        }
        for _ in 0..13 {
            let _ = one.apply(&l1, random_draft(&r2, 30));
        }
    }
    let hash = |e: &Engine<MemStore, ManualClock>| dump_store(&e.store().snapshot(&vault()).unwrap()).unwrap().sha256;
    assert_eq!(hash(&one), hash(&many));
}

#[test]
fn a_batch_reads_the_store_a_fixed_number_of_times_however_many_events_it_holds() {
    let reads_for = |n: usize| {
        let e = engine();
        let lease = e.store().open_vault(&vault()).unwrap();
        let before = e.store().counts().reads.load(Ordering::Relaxed);
        for r in e.apply_batch(&lease, (0..n).map(|i| create(&format!("p{i}"))).collect()) {
            r.unwrap();
        }
        e.store().counts().reads.load(Ordering::Relaxed) - before
    };
    assert_eq!(reads_for(10), reads_for(200));
    assert!(reads_for(200) <= 3);
}

#[test]
fn stale_edit_is_refused_and_current_edit_bumps_the_version() {
    let e = engine();
    let lease = e.store().open_vault(&vault()).unwrap();
    e.apply(&lease, create("p")).unwrap();
    let edit = |base: u32| EventDraft {
        actor: "bob".into(),
        kind: "page.edited".into(),
        schema_version: 1,
        payload: json!({"id": "p", "base_version": base, "body": "new"}),
    };
    e.apply(&lease, edit(1)).unwrap();
    assert!(matches!(
        e.apply(&lease, edit(1)),
        Err(ApplyError::Reduce(ReduceError::StaleVersion { current: 2, base: 1, .. }))
    ));
    let Some(Row::Page(p)) = e.store().snapshot(&vault()).unwrap().get(&page_key("p")).unwrap() else {
        panic!("page")
    };
    assert_eq!((p.header.version, p.body.as_str()), (2, "new"));
}

#[test]
fn time_comes_from_the_injected_clock() {
    let clock = ManualClock::starting_at(1000);
    clock.advance(Duration::from_millis(500));
    let e = Engine::new(MemStore::new(), clock);
    let lease = e.store().open_vault(&vault()).unwrap();
    e.apply(&lease, create("p")).unwrap();
    let snap = e.store().snapshot(&vault()).unwrap();
    assert_eq!(snap.events(None).unwrap()[0].time_ms, 1500);
}

#[test]
fn engine_refuses_unknown_and_corrupt_events_with_no_state_change() {
    let e = engine();
    let lease = e.store().open_vault(&vault()).unwrap();
    let unknown = EventDraft {
        actor: "a".into(),
        kind: "nope".into(),
        schema_version: 1,
        payload: json!({}),
    };
    assert!(matches!(e.apply(&lease, unknown), Err(ApplyError::Upcast(UpcastError::UnknownKind { .. }))));
    let future = EventDraft {
        actor: "a".into(),
        kind: "page.created".into(),
        schema_version: 9,
        payload: json!({}),
    };
    assert!(matches!(e.apply(&lease, future), Err(ApplyError::Upcast(UpcastError::UnknownVersion { .. }))));
    let corrupt = EventDraft {
        actor: "a".into(),
        kind: "page.created".into(),
        schema_version: 1,
        payload: json!({"id": 5}),
    };
    assert!(matches!(e.apply(&lease, corrupt), Err(ApplyError::Reduce(ReduceError::Corrupt { .. }))));
    assert_eq!(e.store().snapshot(&vault()).unwrap().head().unwrap(), None);
}

#[test]
fn replay_stops_at_a_bad_event_and_keeps_only_what_came_before() {
    let target = MemStore::new();
    let lease = target.open_vault(&VaultId("golden".into())).unwrap();
    let mut events = fixture(include_str!("fixtures/events_current.jsonl"));
    events[2].payload = json!({"id": 7});
    let err = replay(events, &target, &lease).unwrap_err();
    assert!(matches!(
        err,
        ReplayError::Reduce {
            seq: 2,
            error: ReduceError::Corrupt { .. }
        }
    ));
    let snap = target.snapshot(&VaultId("golden".into())).unwrap();
    assert_eq!(snap.head().unwrap(), Some(1));
    let Some(Row::Page(p)) = snap.get(&page_key("p1")).unwrap() else {
        panic!("page")
    };
    assert_eq!(p.header.version, 1);
}

#[test]
fn replay_refuses_a_non_contiguous_log() {
    let target = MemStore::new();
    let lease = target.open_vault(&VaultId("golden".into())).unwrap();
    let mut events = fixture(include_str!("fixtures/events_current.jsonl"));
    events.remove(1);
    assert_eq!(
        replay(events, &target, &lease).unwrap_err(),
        ReplayError::Store(StoreError::SequenceGap { head: Some(0), got: 2 })
    );
}

#[test]
fn golden_version_one_events_replay_to_the_same_dump_as_current_ones() {
    let run = |text: &str| {
        let target = MemStore::new();
        let lease = target.open_vault(&VaultId("golden".into())).unwrap();
        assert_eq!(replay(fixture(text), &target, &lease).unwrap(), 5);
        dump_store(&target.snapshot(&VaultId("golden".into())).unwrap()).unwrap()
    };
    let (old, new) = (
        run(include_str!("fixtures/events_v1.jsonl")),
        run(include_str!("fixtures/events_current.jsonl")),
    );
    assert_eq!(old, new);
    let lines: Vec<&str> = old.text.lines().collect();
    assert_eq!(lines[0], "head 4");
    assert_eq!(lines.len(), 5, "{lines:?}");
    assert!(lines.iter().any(|l| l.contains(r#""body":"Andra versionen.""#)
        && l.contains(r#""version":2"#)
        && l.contains(r#""updated_at_ms":3000"#)
        && l.contains("Göteborg")));
    assert!(lines.iter().filter(|l| l.contains(r#""media_type":"application/pdf""#)).count() == 2);
    assert!(lines.iter().any(|l| l.contains(r#""from":"p1""#) && l.contains(r#""to":"s1""#)));
    // Pinned, so a change to rows, the reducer or the dump format shows up here.
    assert_eq!(old.sha256, include_str!("fixtures/golden.sha256").trim());
}

#[test]
fn the_log_keeps_the_original_version_after_replay() {
    let target = MemStore::new();
    let lease = target.open_vault(&VaultId("golden".into())).unwrap();
    replay(fixture(include_str!("fixtures/events_v1.jsonl")), &target, &lease).unwrap();
    let events = all_events(&target.snapshot(&VaultId("golden".into())).unwrap()).unwrap();
    assert_eq!(events[0].schema_version, 1);
    assert_eq!(upcast(&events[0]).unwrap().schema_version, 3);
}

#[test]
fn generated_drafts_are_repeatable() {
    let (a, b) = (SeededRandom::new(9), SeededRandom::new(9));
    for _ in 0..50 {
        assert_eq!(random_draft(&a, 10), random_draft(&b, 10));
    }
}
