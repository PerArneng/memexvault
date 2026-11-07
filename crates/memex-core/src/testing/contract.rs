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
    edges_and_sections_read_in_batches(store, &name("sections"));
    entity_reads(store, &name("entity"));
    page_reads(store, &name("wiki"));
    batches_are_all_or_nothing(store, &name("batch"));
    deleting_a_vault_removes_it(store, &name("gone"));
    vaults_are_isolated(store, &name("iso-a"), &name("iso-b"));
    unknown_vault_is_refused(store, &name("never-opened"));
}

fn page_reads<S: Store>(store: &S, v: &VaultId) {
    use memex_types::{
        BlockKind, BlockRow, CLAIM_REF, EdgeDirection, EdgeRow, Header, PageRow, RevisionBlock, RevisionRow, SlugRow, Status, WIKI_LINK, revision_id,
        slug_target,
    };
    let lease = store.open_vault(v).expect("open");
    let header = |id: &str| Header {
        id: ObjectId(id.into()),
        creator: "t".into(),
        status: Status::Active,
        version: 1,
        acl_id: None,
    };
    let page = |id: &str, slug: &str, redirects: &[&str]| {
        Mutation::Put(Row::Page(PageRow {
            header: header(id),
            title: format!("Title of {id}"),
            body: "a long body".into(),
            updated_at_ms: 5,
            slug: slug.into(),
            redirects: redirects.iter().map(|r| (*r).to_string()).collect(),
        }))
    };
    let block = |id: &str, page: &str, position: u32, kind: BlockKind| {
        Mutation::Put(Row::Block(BlockRow {
            header: header(id),
            page_id: ObjectId(page.into()),
            position,
            kind,
            text: format!("text of {id}"),
            heading: None,
        }))
    };
    let slug = |slug: &str, page: &str| {
        Mutation::Put(Row::Slug(SlugRow {
            header: header(slug),
            page_id: ObjectId(page.into()),
        }))
    };
    let edge = |id: &str, from: &str, to: ObjectId, kind: &str| {
        Mutation::Put(Row::Edge(EdgeRow {
            header: header(id),
            from: ObjectId(from.into()),
            to,
            kind: kind.into(),
            block_id: Some(ObjectId("b1".into())),
        }))
    };
    let revision = |page_id: &str, revision: u32| {
        let page_id = ObjectId(page_id.into());
        Mutation::Put(Row::Revision(RevisionRow {
            header: header(&revision_id(&page_id, revision).0),
            page_id,
            revision,
            title: "t".into(),
            slug: "s".into(),
            blocks: vec![RevisionBlock {
                id: ObjectId("b1".into()),
                kind: BlockKind::Paragraph,
                text: "x".into(),
            }],
            changeset_id: ObjectId("c".into()),
            created_at_ms: 1,
            restored_from: None,
        }))
    };
    let mutations = vec![
        page("p1", "first", &["old-first"]),
        page("p2", "second", &[]),
        block("b2", "p1", 1, BlockKind::Heading),
        block("b1", "p1", 0, BlockKind::Paragraph),
        block("b3", "p2", 0, BlockKind::Paragraph),
        slug("first", "p1"),
        slug("old-first", "p1"),
        slug("second", "p2"),
        edge("e1", "p2", slug_target("old-first"), WIKI_LINK),
        edge("e2", "p2", slug_target("nowhere"), WIKI_LINK),
        edge("e3", "p1", ObjectId("no-claim".into()), CLAIM_REF),
        edge("e4", "p1", slug_target("second"), WIKI_LINK),
        edge("e5", "p1", ObjectId("x".into()), "other"),
        revision("p1", 1),
        revision("p1", 2),
        revision("p2", 1),
    ];
    store.commit(&lease, &raw_event(v, 0), &mutations).expect("commit");
    let snap = store.snapshot(v).expect("snapshot");
    let two = [ObjectId("p1".into()), ObjectId("p2".into())];
    let ids = |b: Vec<BlockRow>| b.into_iter().map(|x| x.header.id.0).collect::<Vec<_>>();
    assert_eq!(ids(snap.blocks_of(&two, 10).expect("blocks")), ["b1", "b2", "b3"], "by page and position");
    assert_eq!(ids(snap.blocks_of(&two, 1).expect("blocks")), ["b1"], "a limit holds");
    assert_eq!(ids(snap.headings_of(&two).expect("headings")), ["b2"]);
    let Some(Row::Page(full)) = snap.get(&page_key("p1")).expect("get") else {
        panic!("no page");
    };
    assert_eq!(
        (full.slug.as_str(), full.redirects.as_slice(), full.body.as_str()),
        ("first", &["old-first".to_string()][..], "a long body")
    );
    let summaries = snap.page_summaries(None, 10).expect("pages");
    assert_eq!(summaries.iter().map(|p| p.header.id.0.as_str()).collect::<Vec<_>>(), ["p1", "p2"]);
    assert!(summaries.iter().all(|p| p.body.is_empty()), "a list carries no bodies");
    assert_eq!(summaries[0].slug, "first");
    assert_eq!(summaries[0].redirects, ["old-first"]);
    assert_eq!(snap.page_summaries(Some(&ObjectId("p1".into())), 10).expect("pages").len(), 1);
    let revs = snap.revisions_of(&ObjectId("p1".into()), None, 10).expect("revisions");
    assert_eq!(revs.iter().map(|r| r.revision).collect::<Vec<_>>(), [1, 2]);
    assert!(revs.iter().all(|r| r.blocks.is_empty()), "a list carries no blocks");
    assert_eq!(snap.revisions_of(&ObjectId("p1".into()), Some(1), 10).expect("revisions").len(), 1);
    let full_revision = snap
        .get(&RowKey {
            table: Table::Revision,
            id: revision_id(&ObjectId("p1".into()), 2),
        })
        .expect("get");
    assert!(matches!(full_revision, Some(Row::Revision(r)) if r.blocks.len() == 1), "a read of one has them");
    let edge_ids = |e: Vec<EdgeRow>| e.into_iter().map(|x| x.header.id.0).collect::<Vec<_>>();
    let p2 = [ObjectId("p2".into())];
    assert_eq!(
        edge_ids(snap.edges_linked(&p2, &[WIKI_LINK], EdgeDirection::From, None, 10).expect("edges")),
        ["e1", "e2"]
    );
    assert_eq!(
        edge_ids(
            snap.edges_linked(&p2, &[WIKI_LINK], EdgeDirection::From, Some(&ObjectId("e1".into())), 10)
                .expect("edges")
        ),
        ["e2"]
    );
    assert_eq!(
        edge_ids(snap.edges_linked(&p2, &[WIKI_LINK], EdgeDirection::From, None, 1).expect("edges")),
        ["e1"]
    );
    let ends = [ObjectId("p1".into()), slug_target("old-first")];
    assert_eq!(
        edge_ids(snap.edges_linked(&ends, &[WIKI_LINK], EdgeDirection::To, None, 10).expect("edges")),
        ["e1"]
    );
    assert_eq!(
        edge_ids(snap.edges_linked(&ends, &[WIKI_LINK, CLAIM_REF], EdgeDirection::Both, None, 10).expect("edges")),
        ["e1", "e3", "e4"]
    );
    let read = snap.edges_linked(&p2, &[WIKI_LINK], EdgeDirection::From, None, 1).expect("edges");
    assert_eq!(read[0].block_id, Some(ObjectId("b1".into())), "an edge keeps its block");
    assert_eq!(
        edge_ids(snap.unresolved_links(None, 10).expect("unresolved")),
        ["e2", "e3"],
        "a slug nobody holds, and a claim that doesn't exist"
    );
    assert_eq!(edge_ids(snap.unresolved_links(Some(&ObjectId("e2".into())), 10).expect("unresolved")), ["e3"]);
    drop(snap);
    store.close_vault(&lease).expect("close");
}

fn entity_reads<S: Store>(store: &S, v: &VaultId) {
    use memex_types::{ClaimedValue, EntityField, EntityRow, EntityTypeRow, FieldFilter, FieldLookup, FieldOp, FilterValue, Header, Status};
    let lease = store.open_vault(v).expect("open");
    let header = |id: &str, status| Header {
        id: ObjectId(id.into()),
        creator: "t".into(),
        status,
        version: 1,
        acl_id: None,
    };
    let text = |t: &str| FieldLookup {
        text: Some(t.into()),
        number: None,
        time_ms: None,
    };
    let number = |n: i64| FieldLookup {
        text: None,
        number: Some(n.into()),
        time_ms: None,
    };
    let time = |t: i64| FieldLookup {
        text: None,
        number: None,
        time_ms: Some(t),
    };
    let field = |name: &str, lookups: Vec<FieldLookup>| EntityField {
        field: name.into(),
        value: json!(1),
        assertion_id: ObjectId(format!("a-{name}")),
        why: "w".into(),
        others: vec![ClaimedValue {
            assertion_id: ObjectId("o".into()),
            value: json!(2),
        }],
        lookups,
    };
    let entity = |id: &str, type_name: &str, name: &str, price: i64, at: i64, aliases: &[&str]| EntityRow {
        header: header(id, Status::Active),
        type_name: type_name.into(),
        type_version: 1,
        aliases: aliases.iter().map(|a| (*a).to_string()).collect(),
        alias_keys: aliases.iter().map(|a| (*a).to_string()).collect(),
        fields: vec![
            field("name", vec![text(name)]),
            field("price", vec![number(price)]),
            field("at", vec![time(at)]),
            field("tags", vec![text("red"), text("blue")]),
        ],
        merged_from: vec![],
        redirect_to: None,
        changeset_id: ObjectId("c".into()),
        updated_at_ms: 0,
    };
    let mut gone = entity("e9", "T", "pump", 1, 1, &["old"]);
    gone.header.status = Status::Superseded;
    gone.redirect_to = Some(ObjectId("e1".into()));
    let ty = |name: &str, version_no: u32| EntityTypeRow {
        header: header(&format!("{name}@{version_no:06}"), Status::Active),
        name: name.into(),
        version_no,
        schema: json!({"v": version_no}),
        changeset_id: None,
        created_at_ms: 0,
        change: "new".into(),
        reasons: vec![],
    };
    let claim = |id: &str, subject: &str, status| memex_types::AssertionRow {
        header: header(id, status),
        subject: subject.into(),
        predicate: "name".into(),
        value: json!("v"),
        epistemic: memex_types::Epistemic::Asserted,
        machine: false,
        confidence_permille: 0,
        valid_from_ms: None,
        valid_to_ms: None,
        canonical_hash: id.into(),
        changeset_id: ObjectId("c".into()),
        superseded_by: None,
    };
    let put = |r: Row| Mutation::Put(r);
    let mutations = vec![
        put(Row::EntityType(ty("T", 1))),
        put(Row::EntityType(ty("T", 2))),
        put(Row::EntityType(ty("U", 1))),
        put(Row::Entity(entity("e1", "T", "pump", 10, 100, &["acme"]))),
        put(Row::Entity(entity("e2", "T", "valve", 20, 200, &["acme"]))),
        put(Row::Entity(entity("e3", "T", "pump", 30, 300, &[]))),
        put(Row::Entity(entity("e4", "U", "pump", 10, 100, &[]))),
        put(Row::Entity(gone)),
        put(Row::Assertion(claim("a1", "entity:e1", Status::Active))),
        put(Row::Assertion(claim("a2", "entity:e2", Status::Active))),
        put(Row::Assertion(claim("a3", "entity:e1", Status::Superseded))),
    ];
    let event = Event {
        seq: 0,
        vault: v.clone(),
        actor: "t".into(),
        time_ms: 1,
        kind: "state.recorded".into(),
        schema_version: 1,
        payload: json!({"mutations": mutations}),
    };
    store.commit(&lease, &event, &mutations).expect("commit");
    let s = store.snapshot(v).expect("snapshot");
    let ids = |rows: Vec<EntityRow>| rows.into_iter().map(|r| r.header.id.0).collect::<Vec<_>>();
    let filter = |f: &str, op, value| FieldFilter { field: f.into(), op, value };
    // Types: versions in order, and the newest of each.
    let versions: Vec<u32> = s.entity_type_versions(&["T".to_string()]).unwrap().iter().map(|t| t.version_no).collect();
    assert_eq!(versions, [1, 2]);
    let latest = s.latest_entity_types(None, 10).unwrap();
    assert_eq!(latest.iter().map(|t| (t.name.as_str(), t.version_no)).collect::<Vec<_>>(), [("T", 2), ("U", 1)]);
    assert_eq!(s.latest_entity_types(Some("T"), 10).unwrap().len(), 1);
    assert_eq!(s.latest_entity_types(None, 1).unwrap().len(), 1);
    // A merged entity is never current.
    assert_eq!(ids(s.entities_of_type("T", None, 10).unwrap()), ["e1", "e2", "e3"]);
    assert_eq!(ids(s.entities_of_type("T", Some(&ObjectId("e1".into())), 1).unwrap()), ["e2"]);
    // Exact, range, several together, an array field, and other types stay out.
    let eq = |t: &str| filter("name", FieldOp::Eq, FilterValue::Text(t.into()));
    assert_eq!(ids(s.entities_matching("T", &[eq("pump")], None, 10).unwrap()), ["e1", "e3"]);
    assert_eq!(ids(s.entities_matching("U", &[eq("pump")], None, 10).unwrap()), ["e4"]);
    assert_eq!(ids(s.entities_matching("T", &[eq("pump")], Some(&ObjectId("e1".into())), 10).unwrap()), ["e3"]);
    assert_eq!(ids(s.entities_matching("T", &[eq("pump")], None, 1).unwrap()), ["e1"]);
    let price = |op, n| filter("price", op, FilterValue::Number(n));
    assert_eq!(ids(s.entities_matching("T", &[price(FieldOp::Lt, 20.0)], None, 10).unwrap()), ["e1"]);
    assert_eq!(ids(s.entities_matching("T", &[price(FieldOp::Lte, 20.0)], None, 10).unwrap()), ["e1", "e2"]);
    assert_eq!(ids(s.entities_matching("T", &[price(FieldOp::Gt, 20.0)], None, 10).unwrap()), ["e3"]);
    assert_eq!(ids(s.entities_matching("T", &[price(FieldOp::Gte, 20.0)], None, 10).unwrap()), ["e2", "e3"]);
    assert_eq!(
        ids(s.entities_matching("T", &[eq("pump"), price(FieldOp::Gt, 20.0)], None, 10).unwrap()),
        ["e3"]
    );
    let at = filter("at", FieldOp::Gte, FilterValue::Time(200));
    assert_eq!(ids(s.entities_matching("T", &[at], None, 10).unwrap()), ["e2", "e3"]);
    let tag = filter("tags", FieldOp::Eq, FilterValue::Text("red".into()));
    assert_eq!(
        ids(s.entities_matching("T", &[tag], None, 10).unwrap()),
        ["e1", "e2", "e3"],
        "an entity with a matching array item, once each"
    );
    // Lookup by value, which sees aliases and not merged entities.
    let m = s.entities_with_value("acme", 10).unwrap();
    assert_eq!(
        m.iter().map(|x| (x.entity_id.0.as_str(), x.field.as_str())).collect::<Vec<_>>(),
        [("e1", "$alias"), ("e2", "$alias")]
    );
    assert!(s.entities_with_value("old", 10).unwrap().is_empty());
    assert_eq!(s.entities_with_value("pump", 2).unwrap().len(), 2);
    // Shared values.
    let fields = ["name".to_string(), "$alias".to_string()];
    let shared = s.shared_values("T", &fields, None, 10).unwrap();
    let as_pairs: Vec<(&str, &str, usize)> = shared.iter().map(|x| (x.field.as_str(), x.text.as_str(), x.entity_ids.len())).collect();
    assert_eq!(as_pairs, [("$alias", "acme", 2), ("name", "pump", 2)]);
    assert_eq!(s.shared_values("T", &fields, Some(("$alias", "acme")), 10).unwrap().len(), 1);
    assert_eq!(s.shared_values("T", &fields, None, 1).unwrap().len(), 1);
    // Claims about subjects.
    let about: Vec<String> = s
        .assertions_about(&["entity:e1".to_string()])
        .unwrap()
        .into_iter()
        .map(|a| a.header.id.0)
        .collect();
    assert_eq!(about, ["a1"], "only active ones");
    drop(s);
    // A row's values follow it: replacing a row replaces them, and deleting it removes them.
    let mutations = vec![
        put(Row::Entity(entity("e1", "T", "changed", 10, 100, &[]))),
        Mutation::Delete(RowKey {
            table: Table::Entity,
            id: ObjectId("e3".into()),
        }),
    ];
    let event = Event {
        seq: 1,
        payload: json!({"mutations": mutations}),
        ..event
    };
    store.commit(&lease, &event, &mutations).expect("commit");
    let s = store.snapshot(v).expect("snapshot");
    assert!(s.entities_matching("T", &[eq("pump")], None, 10).unwrap().is_empty());
    assert_eq!(ids(s.entities_matching("T", &[eq("changed")], None, 10).unwrap()), ["e1"]);
    drop(s);
    store.close_vault(&lease).expect("close");
}

fn edges_and_sections_read_in_batches<S: Store>(store: &S, v: &VaultId) {
    let lease = store.open_vault(v).expect("open");
    let header = |id: &str| memex_types::Header {
        id: ObjectId(id.into()),
        creator: "t".into(),
        status: memex_types::Status::Active,
        version: 1,
        acl_id: None,
    };
    let edge = |id: &str, from: &str, to: &str, kind: &str| {
        Mutation::Put(Row::Edge(memex_types::EdgeRow {
            header: header(id),
            from: ObjectId(from.into()),
            to: ObjectId(to.into()),
            kind: kind.into(),
            block_id: None,
        }))
    };
    let segment = |id: &str, rendition: &str, heading: Option<&str>, start: u64| {
        Mutation::Put(Row::Segment(memex_types::SegmentRow {
            header: header(id),
            source_id: ObjectId("src".into()),
            language: None,
            text: format!("text of {id}"),
            rendition_id: Some(ObjectId(rendition.into())),
            start,
            end: start + 1,
            heading: heading.map(String::from),
            page_from: None,
            page_to: None,
        }))
    };
    let rows = vec![
        edge("e1", "a", "b", "contradicts"),
        edge("e2", "c", "a", "contradicts"),
        edge("e3", "a", "d", "cites"),
        edge("e4", "x", "y", "contradicts"),
        segment("z-first", "r1", Some("H"), 0),
        segment("a-second", "r1", Some("H"), 10),
        segment("other", "r1", Some("J"), 20),
        segment("none", "r1", None, 30),
        segment("elsewhere", "r2", Some("H"), 0),
    ];
    store.commit(&lease, &raw_event(v, 0), &rows).expect("commit");
    let snap = store.snapshot(v).expect("snap");
    let mut touching: Vec<String> = snap
        .edges_touching(&[ObjectId("a".into()), ObjectId("nope".into())], "contradicts")
        .expect("edges")
        .into_iter()
        .map(|e| e.header.id.0)
        .collect();
    touching.sort();
    assert_eq!(touching, ["e1", "e2"]);
    let ids = |rows: Vec<memex_types::SegmentRow>| rows.into_iter().map(|s| s.header.id.0).collect::<Vec<_>>();
    assert_eq!(
        ids(snap.sections_of(&[(ObjectId("r1".into()), "H".into())], 10).expect("section")),
        ["z-first", "a-second"],
        "by offset, not id"
    );
    assert_eq!(
        ids(snap
            .sections_of(&[(ObjectId("r2".into()), "H".into()), (ObjectId("r1".into()), "J".into())], 10)
            .expect("two")),
        ["other", "elsewhere"],
        "only the pairs asked for"
    );
    assert_eq!(ids(snap.sections_of(&[(ObjectId("r1".into()), "H".into())], 1).expect("limit")), ["z-first"]);
    assert!(snap.sections_of(&[(ObjectId("r9".into()), "H".into())], 10).expect("none").is_empty());
    drop(snap);
    store.close_vault(&lease).expect("close");
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
