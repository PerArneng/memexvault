//! The contract any Store must meet. Spec 032 runs the same functions against Postgres, so nothing here may assume MemStore.
use crate::{
    ApplyError, CommitItem, Engine, ReduceError, Snapshot, Store, StoreError, all_events, dump_store, replay,
    testing::{ManualClock, SeededRandom, generate::random_draft},
};
use memex_types::{Event, EventDraft, Mutation, ObjectId, Row, RowKey, Table, VaultId};
use serde_json::json;

fn vault(name: &str) -> VaultId {
    VaultId(name.into())
}

fn page(id: &str, version_body: &str) -> EventDraft {
    EventDraft {
        actor: "t".into(),
        kind: "page.created".into(),
        schema_version: 1,
        payload: json!({"id": id, "title": "T", "body": version_body}),
    }
}

fn page_key(id: &str) -> RowKey {
    RowKey {
        table: Table::Page,
        id: ObjectId(id.into()),
    }
}

fn raw_event(v: &VaultId, seq: u64) -> Event {
    Event {
        seq,
        vault: v.clone(),
        actor: "t".into(),
        time_ms: 1,
        kind: "page.deleted".into(),
        schema_version: 1,
        payload: json!({"id": "x"}),
    }
}

/// Runs every contract check, each on a fresh vault in `store`.
pub fn run_all<S: Store>(store: &S, prefix: &str) {
    let name = |n: &str| vault(&format!("{prefix}{n}"));
    apply_then_read(store, &name("apply"));
    writer_lease_is_exclusive(store, &name("lease"));
    sequences_must_be_contiguous(store, &name("seq"));
    missing_delete_leaves_no_trace(store, &name("delete"));
    snapshot_ignores_later_commits(store, &name("snap"));
    get_many_keeps_key_order(store, &name("many"));
    paging_returns_everything_once(store, &name("page"));
    compact_changes_no_reads(store, &name("compact"));
    batches_are_all_or_nothing(store, &name("batch"));
    deleting_a_vault_removes_it(store, &name("gone"));
    vaults_are_isolated(store, &name("iso-a"), &name("iso-b"));
    unknown_vault_is_refused(store, &name("never-opened"));
}

fn apply_then_read<S: Store>(store: &S, v: &VaultId) {
    let lease = store.open_vault(v).expect("open");
    let c = ManualClock::starting_at(5);
    let snap = store.snapshot(v).expect("snap");
    assert_eq!(snap.head().expect("head"), None);
    drop(snap);
    let event = Event {
        seq: 0,
        vault: v.clone(),
        actor: "t".into(),
        time_ms: crate::Clock::now_ms(&c),
        kind: "page.created".into(),
        schema_version: 1,
        payload: json!({"id": "p1", "title": "Å", "body": "ñ"}),
    };
    let row = crate::reduce(&store.snapshot(v).expect("snap"), &event).expect("reduce");
    assert_eq!(store.commit(&lease, &event, &row).expect("commit").seq, 0);
    let snap = store.snapshot(v).expect("snap");
    assert_eq!(snap.head().expect("head"), Some(0));
    let Some(Row::Page(p)) = snap.get(&page_key("p1")).expect("get") else {
        panic!("page")
    };
    assert_eq!((p.title.as_str(), p.body.as_str(), p.header.version, p.updated_at_ms), ("Å", "ñ", 1, 5));
    assert_eq!(snap.events(None).expect("events"), vec![event]);
    drop(snap);
    store.close_vault(&lease).expect("close");
}

fn writer_lease_is_exclusive<S: Store>(store: &S, v: &VaultId) {
    let lease = store.open_vault(v).expect("open");
    assert!(matches!(store.open_vault(v), Err(StoreError::WriterActive { .. })));
    store.close_vault(&lease).expect("close");
    assert_eq!(store.commit(&lease, &raw_event(v, 0), &[]), Err(StoreError::LeaseLost));
    let second = store.open_vault(v).expect("reopen");
    assert_eq!(store.commit(&lease, &raw_event(v, 0), &[]), Err(StoreError::LeaseLost));
    store.close_vault(&second).expect("close");
}

fn sequences_must_be_contiguous<S: Store>(store: &S, v: &VaultId) {
    let lease = store.open_vault(v).expect("open");
    assert_eq!(store.commit(&lease, &raw_event(v, 1), &[]), Err(StoreError::SequenceGap { head: None, got: 1 }));
    store.commit(&lease, &raw_event(v, 0), &[]).expect("first");
    assert_eq!(
        store.commit(&lease, &raw_event(v, 0), &[]),
        Err(StoreError::SequenceGap { head: Some(0), got: 0 })
    );
    assert_eq!(
        store.commit(&lease, &raw_event(v, 5), &[]),
        Err(StoreError::SequenceGap { head: Some(0), got: 5 })
    );
    assert_eq!(store.snapshot(v).expect("snap").head().expect("head"), Some(0));
    store.close_vault(&lease).expect("close");
}

fn missing_delete_leaves_no_trace<S: Store>(store: &S, v: &VaultId) {
    let lease = store.open_vault(v).expect("open");
    let before = dump_store(&store.snapshot(v).expect("snap")).expect("dump");
    let mutations = [Mutation::Delete(page_key("ghost"))];
    assert!(matches!(store.commit(&lease, &raw_event(v, 0), &mutations), Err(StoreError::MissingRow { .. })));
    let snap = store.snapshot(v).expect("snap");
    assert_eq!(dump_store(&snap).expect("dump"), before);
    assert!(snap.events(None).expect("events").is_empty());
    drop(snap);
    store.close_vault(&lease).expect("close");
}

fn apply_all<S: Store>(store: &S, lease: &crate::WriterLease, drafts: Vec<EventDraft>) {
    let v = &lease.vault;
    for (i, d) in drafts.into_iter().enumerate() {
        let seq = store.snapshot(v).expect("snap").head().expect("head").map_or(0, |h| h + 1);
        let e = Event {
            seq,
            vault: v.clone(),
            actor: d.actor,
            time_ms: i as i64,
            kind: d.kind,
            schema_version: d.schema_version,
            payload: d.payload,
        };
        let m = crate::reduce(&store.snapshot(v).expect("snap"), &e).expect("reduce");
        store.commit(lease, &e, &m).expect("commit");
    }
}

fn snapshot_ignores_later_commits<S: Store>(store: &S, v: &VaultId) {
    let lease = store.open_vault(v).expect("open");
    apply_all(store, &lease, vec![page("p1", "one")]);
    let old = store.snapshot(v).expect("snap");
    apply_all(store, &lease, vec![page("p2", "two")]);
    assert_eq!(old.head().expect("head"), Some(0));
    assert_eq!(old.get(&page_key("p2")).expect("get"), None);
    drop(old);
    assert!(store.snapshot(v).expect("snap").get(&page_key("p2")).expect("get").is_some());
    store.close_vault(&lease).expect("close");
}

fn get_many_keeps_key_order<S: Store>(store: &S, v: &VaultId) {
    let lease = store.open_vault(v).expect("open");
    apply_all(store, &lease, vec![page("a", ""), page("b", "")]);
    let got = store
        .snapshot(v)
        .expect("snap")
        .get_many(&[page_key("b"), page_key("ghost"), page_key("a"), page_key("b")])
        .expect("many");
    let ids: Vec<Option<String>> = got.iter().map(|r| r.as_ref().map(|x| x.header().id.0.clone())).collect();
    assert_eq!(ids, vec![Some("b".into()), None, Some("a".into()), Some("b".into())]);
    assert!(store.snapshot(v).expect("snap").get_many(&[]).expect("empty").is_empty());
    store.close_vault(&lease).expect("close");
}

fn paging_returns_everything_once<S: Store>(store: &S, v: &VaultId) {
    let lease = store.open_vault(v).expect("open");
    let n = crate::PAGE_LIMIT + 5;
    apply_all(store, &lease, (0..n).map(|i| page(&format!("p{i:06}"), "x")).collect());
    let snap = store.snapshot(v).expect("snap");
    assert_eq!(all_events(&snap).expect("events").len(), n);
    let first = snap.rows(Table::Page, None).expect("rows");
    assert_eq!(first.len(), crate::PAGE_LIMIT);
    let rest = snap.rows(Table::Page, Some(&first[first.len() - 1].header().id)).expect("rows");
    assert_eq!(rest.len(), 5);
    assert!(snap.rows(Table::Source, None).expect("rows").is_empty());
    drop(snap);
    store.close_vault(&lease).expect("close");
}

fn compact_changes_no_reads<S: Store>(store: &S, v: &VaultId) {
    let lease = store.open_vault(v).expect("open");
    let del = EventDraft {
        actor: "t".into(),
        kind: "page.deleted".into(),
        schema_version: 1,
        payload: json!({"id": "a"}),
    };
    apply_all(store, &lease, vec![page("a", "gone"), page("b", "kept"), del]);
    let before = dump_store(&store.snapshot(v).expect("snap")).expect("dump");
    store.compact(&lease).expect("compact");
    assert_eq!(dump_store(&store.snapshot(v).expect("snap")).expect("dump"), before);
    assert_eq!(store.snapshot(v).expect("snap").get(&page_key("a")).expect("get"), None);
    store.close_vault(&lease).expect("close");
}

fn batches_are_all_or_nothing<S: Store>(store: &S, v: &VaultId) {
    let lease = store.open_vault(v).expect("open");
    assert_eq!(store.commit_batch(&lease, &[]), Err(StoreError::EmptyBatch));
    apply_all(store, &lease, vec![page("a", "kept")]);
    let (e1, e2, e_gap) = (raw_event(v, 1), raw_event(v, 2), raw_event(v, 9));
    let put = {
        let snap = store.snapshot(v).expect("snap");
        let Some(row) = snap.get(&page_key("a")).expect("get") else { panic!("a") };
        let Row::Page(mut p) = row else { panic!("page") };
        p.header.id = ObjectId("new".into());
        Mutation::Put(Row::Page(p))
    };
    let before = dump_store(&store.snapshot(v).expect("snap")).expect("dump");
    // A gap late in the batch and a delete of a missing row both refuse everything.
    let refused = [
        CommitItem {
            event: &e1,
            mutations: std::slice::from_ref(&put),
        },
        CommitItem { event: &e_gap, mutations: &[] },
    ];
    assert_eq!(store.commit_batch(&lease, &refused), Err(StoreError::SequenceGap { head: Some(1), got: 9 }));
    let missing = [Mutation::Delete(page_key("ghost"))];
    let refused = [
        CommitItem {
            event: &e1,
            mutations: std::slice::from_ref(&put),
        },
        CommitItem {
            event: &e2,
            mutations: &missing,
        },
    ];
    assert!(matches!(store.commit_batch(&lease, &refused), Err(StoreError::MissingRow { .. })));
    assert_eq!(dump_store(&store.snapshot(v).expect("snap")).expect("dump"), before);
    // A row put and deleted within one batch, and a row deleted then put again, end up right.
    let del_new = [Mutation::Delete(page_key("new"))];
    let del_a = [Mutation::Delete(page_key("a"))];
    let again = [Mutation::Delete(page_key("a"))];
    let accepted = [
        CommitItem {
            event: &e1,
            mutations: std::slice::from_ref(&put),
        },
        CommitItem {
            event: &e2,
            mutations: &del_new,
        },
        CommitItem {
            event: &raw_event(v, 3),
            mutations: &del_a,
        },
    ];
    assert_eq!(store.commit_batch(&lease, &accepted).expect("batch").seq, 3);
    let snap = store.snapshot(v).expect("snap");
    assert_eq!(snap.head().expect("head"), Some(3));
    assert_eq!(snap.get(&page_key("new")).expect("get"), None);
    assert_eq!(snap.get(&page_key("a")).expect("get"), None);
    assert_eq!(snap.events(Some(0)).expect("events").len(), 3);
    drop(snap);
    let double = [CommitItem {
        event: &raw_event(v, 4),
        mutations: &again,
    }];
    assert!(matches!(store.commit_batch(&lease, &double), Err(StoreError::MissingRow { .. })));
    store.close_vault(&lease).expect("close");
}

fn deleting_a_vault_removes_it<S: Store>(store: &S, v: &VaultId) {
    let lease = store.open_vault(v).expect("open");
    apply_all(store, &lease, vec![page("p", "x")]);
    store.delete_vault(&lease).expect("delete");
    assert!(matches!(store.snapshot(v), Err(StoreError::UnknownVault { .. })));
    assert_eq!(store.commit(&lease, &raw_event(v, 0), &[]), Err(StoreError::LeaseLost));
    let again = store.open_vault(v).expect("reopen creates it empty");
    assert_eq!(store.snapshot(v).expect("snap").head().expect("head"), None);
    store.delete_vault(&again).expect("delete");
}

fn vaults_are_isolated<S: Store>(store: &S, a: &VaultId, b: &VaultId) {
    let (la, lb) = (store.open_vault(a).expect("a"), store.open_vault(b).expect("b"));
    apply_all(store, &la, vec![page("only-a", "x")]);
    assert_eq!(store.snapshot(b).expect("snap").get(&page_key("only-a")).expect("get"), None);
    assert_eq!(store.snapshot(b).expect("snap").head().expect("head"), None);
    store.close_vault(&la).expect("close");
    store.close_vault(&lb).expect("close");
}

fn unknown_vault_is_refused<S: Store>(store: &S, v: &VaultId) {
    assert!(matches!(store.snapshot(v), Err(StoreError::UnknownVault { .. })));
}

/// Applies `n` random events through the engine, replays the log into `target`, and returns (applied, refused, source hash, target hash).
pub fn random_replay<A: Store, B: Store>(source: A, target: &B, target_vault: &VaultId, seed: u64, n: u64) -> (u64, u64, String, String) {
    let v = vault("random-source");
    let lease = source.open_vault(&v).expect("open");
    let clock = ManualClock::starting_at(1_700_000_000_000);
    let rng = SeededRandom::new(seed);
    let engine = Engine::new(source, clock);
    let (mut applied, mut refused) = (0, 0);
    for _ in 0..n {
        match engine.apply(&lease, random_draft(&rng, 2000)) {
            Ok(_) => applied += 1,
            Err(ApplyError::Reduce(ReduceError::AlreadyExists(_) | ReduceError::NotFound(_) | ReduceError::StaleVersion { .. })) => refused += 1,
            Err(e) => panic!("unexpected: {e}"),
        }
    }
    let snap = engine.store().snapshot(&v).expect("snap");
    let source_hash = dump_store(&snap).expect("dump").sha256;
    let mut events = all_events(&snap).expect("events");
    for e in &mut events {
        e.vault = target_vault.clone();
    }
    let target_lease = target.open_vault(target_vault).expect("open");
    let replayed = replay(events, target, &target_lease).expect("replay");
    assert_eq!(replayed, applied);
    let target_hash = dump_store(&target.snapshot(target_vault).expect("snap")).expect("dump").sha256;
    target.close_vault(&target_lease).expect("close");
    (applied, refused, source_hash, target_hash)
}
