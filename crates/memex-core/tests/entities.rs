//! The entity registry through the facade, in memory.
use memex_core::testing::{ManualClock, MemBlobStore, MemRegistry, MemStore, SeededRandom};
use memex_core::{Caller, EntityEdit, FieldInput, NewEntity, NewToken, Proposal, Server, ServerEdges, ServerError, Snapshot, SplitRequest, StepOutcome, Store};
use memex_types::{
    ChangeSetState, DocKind, EvidenceInput, Grant, ItemKind, LexicalFilter, ObjectId, Op, PrincipalKind, RiskDecision, Row, RowKey, Scope, SearchMode,
    SearchRequest, Stance, Table, VaultId,
};
use serde_json::{Value, json};
use std::sync::Arc;

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
}

struct World {
    s: Arc<Server<Fakes>>,
    admin: Caller,
    human: Caller,
    agent: Caller,
    agent_admin: Caller,
    reader: Caller,
    v: VaultId,
}

fn world() -> World {
    let s = Arc::new(Server::<Fakes>::new(
        MemRegistry::new(),
        MemStore::new(),
        MemBlobStore::new(ManualClock::starting_at(0)),
        ManualClock::starting_at(1_000_000),
        SeededRandom::new(1),
        memex_core::testing::mem_search(),
    ));
    let admin = s.authenticate(&s.bootstrap("root").unwrap().token).unwrap();
    let v = s.create_vault(&admin, "v").unwrap().id;
    let make = |name: &str, kind, scope| {
        let p = s.create_principal(&admin, name, kind, false).unwrap();
        let t = s
            .create_token(
                &admin,
                NewToken {
                    principal_id: Some(p.id),
                    name: "t".into(),
                    grants: vec![Grant { vault: v.clone(), scope }],
                    expires_at_ms: None,
                },
            )
            .unwrap();
        s.authenticate(&t.token).unwrap()
    };
    let (human, agent, agent_admin, reader) = (
        make("human", PrincipalKind::Human, Scope::Propose),
        make("agent", PrincipalKind::Agent, Scope::Propose),
        make("agent-admin", PrincipalKind::Agent, Scope::Admin),
        make("reader", PrincipalKind::Human, Scope::Read),
    );
    s.record_segment(
        &admin,
        &v,
        &ObjectId("seg1".into()),
        &ObjectId("src1".into()),
        None,
        "The pump weighs 12 kg and costs 90 euro. The valve costs 30 euro.",
    )
    .unwrap();
    World {
        s,
        admin,
        human,
        agent,
        agent_admin,
        reader,
        v,
    }
}

fn widget_v1() -> Value {
    json!({"type": "object", "additionalProperties": false, "required": ["name"], "properties": {
        "name": {"type": "string", "identity": true, "searchable": true, "filterable": true},
        "price": {"type": "number", "filterable": true}
    }})
}

fn with_color() -> Value {
    let mut s = widget_v1();
    s["properties"]["color"] = json!({"type": "string", "searchable": true});
    s
}

fn field(name: &str, value: Value) -> FieldInput {
    FieldInput {
        field: name.into(),
        value,
        evidence: vec![],
        replaces: None,
    }
}

fn cited(name: &str, value: Value, quote: &str) -> FieldInput {
    FieldInput {
        field: name.into(),
        value,
        evidence: vec![EvidenceInput {
            segment_id: ObjectId("seg1".into()),
            quote: quote.into(),
            stance: Stance::Supports,
        }],
        replaces: None,
    }
}

fn define(w: &World, who: &Caller, name: &str, base: Option<u32>, schema: Value, key: &str) -> memex_core::ProposalOutcome {
    w.s.propose_entity_type(who, &w.v, name, base, schema, "r".into(), key.into()).unwrap()
}

fn state(o: &memex_core::ProposalOutcome) -> ChangeSetState {
    o.changeset.as_ref().unwrap().state
}

fn cs_id(o: &memex_core::ProposalOutcome) -> ObjectId {
    o.changeset.as_ref().unwrap().header.id.clone()
}

fn new_widget(w: &World, who: &Caller, key: &str, fields: Vec<FieldInput>) -> (ObjectId, memex_core::ProposalOutcome) {
    w.s.propose_new_entity(
        who,
        &w.v,
        NewEntity {
            type_name: "Widget".into(),
            aliases: vec![],
            fields,
            rationale: "r".into(),
            idempotency_key: key.into(),
        },
        false,
    )
    .unwrap()
}

fn widget(w: &World, key: &str, name: &str) -> ObjectId {
    let (id, o) = new_widget(w, &w.human, key, vec![field("name", json!(name))]);
    assert_eq!(state(&o), ChangeSetState::Committed, "{:?}", o.validators);
    id
}

fn project(w: &World) {
    for _ in 0..10_000 {
        w.s.engine().clock().advance(std::time::Duration::from_secs(1));
        let a = w.s.project_lexical(&w.v).unwrap();
        let b = w.s.project_vector(&w.v).unwrap();
        if a == StepOutcome::Idle && b == StepOutcome::Idle {
            return;
        }
    }
    panic!("the projectors never settled");
}

fn run_embeds(w: &World) {
    while let Some(job) = w.s.claim_job(&w.v, memex_core::EMBED_JOB, "w").unwrap() {
        w.s.run_embed_job(&w.v, &job).unwrap();
        assert!(w.s.complete_job(&w.v, &job, vec![]).unwrap());
    }
}

fn ids(items: &[memex_types::EvidenceItem], kind: ItemKind) -> Vec<String> {
    items.iter().filter(|i| i.kind == kind).map(|i| i.id.0.clone()).collect()
}

/// Verification 1, with fakes: a brand-new type, found every way, by a server that never restarted.
#[test]
fn a_new_type_is_found_by_text_exact_and_filter_with_no_restart() {
    let w = world();
    project(&w);
    assert_eq!(state(&define(&w, &w.human, "Widget", None, with_color(), "t")), ChangeSetState::Committed);
    let (id, o) = new_widget(
        &w,
        &w.human,
        "e",
        vec![
            field("name", json!("Zephyr Pump")),
            field("price", json!(90)),
            field("color", json!("vermilion")),
        ],
    );
    assert_eq!(state(&o), ChangeSetState::Committed);
    let other = widget(&w, "e2", "Plain Valve");
    project(&w);
    run_embeds(&w);
    project(&w);
    // Text, through the one search API and both signals.
    for mode in [SearchMode::Hybrid, SearchMode::Lexical, SearchMode::Vector] {
        let mut r = SearchRequest::new("vermilion Zephyr");
        r.mode = mode;
        let p = w.s.search(&w.admin, &w.v, &r).unwrap();
        assert!(ids(&p.items, ItemKind::Entity).contains(&id.0), "{mode:?}: {:?}", p.items);
    }
    // Exact id, and exact identity value as the third signal.
    let p = w.s.search(&w.admin, &w.v, &SearchRequest::new(id.0.clone())).unwrap();
    assert_eq!(p.path, memex_types::QueryPath::Exact);
    assert_eq!(ids(&p.items, ItemKind::Entity), std::slice::from_ref(&id.0));
    let mut r = SearchRequest::new("zephyr   pump");
    r.mode = SearchMode::Vector;
    assert!(ids(&w.s.search(&w.admin, &w.v, &r).unwrap().items, ItemKind::Entity).contains(&id.0));
    // Filtered: by type in search, by field through the filter API, and within one field.
    let mut r = SearchRequest::new("valve pump");
    r.filter.types = vec!["Widget".into()];
    r.filter.kinds = vec![DocKind::Entity];
    let found = ids(&w.s.search(&w.admin, &w.v, &r).unwrap().items, ItemKind::Entity);
    assert_eq!(found.len(), 2, "{found:?}");
    let f = |field: &str, op: &str, value: &str| (field.to_string(), op.to_string(), value.to_string());
    let under = w.s.list_entities(&w.admin, &w.v, "Widget", &[f("price", "lt", "100")], None, 10).unwrap();
    assert_eq!(under.iter().map(|e| e.header.id.clone()).collect::<Vec<_>>(), std::slice::from_ref(&id));
    let named =
        w.s.list_entities(&w.admin, &w.v, "Widget", &[f("name", "eq", "PLAIN  valve")], None, 10)
            .unwrap();
    assert_eq!(named[0].header.id, other);
    let mut r = SearchRequest::new("vermilion");
    r.filter.attr = Some(memex_types::AttrScope {
        type_name: "Widget".into(),
        field: "color".into(),
    });
    let p = w.s.search(&w.admin, &w.v, &r).unwrap();
    assert_eq!(ids(&p.items, ItemKind::Entity), std::slice::from_ref(&id.0));
    r.filter.attr = Some(memex_types::AttrScope {
        type_name: "Widget".into(),
        field: "name".into(),
    });
    assert!(ids(&w.s.search(&w.admin, &w.v, &r).unwrap().items, ItemKind::Entity).is_empty());
    r.mode = SearchMode::Vector;
    assert!(matches!(w.s.search(&w.admin, &w.v, &r), Err(ServerError::Invalid(_))));
    // Rebuilding the indexes gives the same answer.
    w.s.rebuild_lexical(&w.v);
    project(&w);
    let p = w.s.search(&w.admin, &w.v, &SearchRequest::new("vermilion")).unwrap();
    assert!(ids(&p.items, ItemKind::Entity).contains(&id.0));
}

/// Verification 2: the current value and its evidence, in search as well.
#[test]
fn a_search_hit_on_an_entity_cites_the_evidence_behind_its_values() {
    let w = world();
    define(&w, &w.human, "Widget", None, widget_v1(), "t");
    let (id, _) = new_widget(
        &w,
        &w.human,
        "e",
        vec![cited("name", json!("Zephyr"), "The pump"), cited("price", json!(90), "costs 90 euro")],
    );
    project(&w);
    let p = w.s.search(&w.admin, &w.v, &SearchRequest::new("Zephyr")).unwrap();
    let item = p.items.iter().find(|i| i.id == id).unwrap();
    assert_eq!(item.citations.len(), 2);
    assert!(
        item.citations
            .iter()
            .all(|c| c.segment_id == Some(ObjectId("seg1".into())) && c.evidence_id.is_some())
    );
    assert_eq!(item.heading.as_deref(), Some("Widget"));
}

/// Verification 3.
#[test]
fn conflicting_values_stay_visible_with_the_choice_and_why() {
    let w = world();
    define(&w, &w.human, "Widget", None, widget_v1(), "t");
    let (id, _) = new_widget(&w, &w.human, "e", vec![field("name", json!("Z")), cited("price", json!(90), "costs 90 euro")]);
    let edit = |key: &str, base, fields| EntityEdit {
        base_version: base,
        aliases: None,
        fields,
        rationale: "r".into(),
        idempotency_key: key.to_string(),
    };
    let o =
        w.s.propose_entity_edit(&w.human, &w.v, &id, edit("e2", 1, vec![cited("price", json!(30), "costs 30 euro")]), false)
            .unwrap();
    assert_eq!(state(&o), ChangeSetState::Committed);
    assert_eq!(o.contradictions.len(), 1);
    let view = w.s.get_entity(&w.admin, &w.v, &id).unwrap();
    let price = view.entity.field("price").unwrap();
    assert_eq!(price.value, json!(30));
    assert!(price.why.contains("newer"), "{}", price.why);
    assert_eq!(price.others.len(), 1);
    assert_eq!(price.others[0].value, json!(90));
    assert_eq!(view.claims[&price.assertion_id].evidence[0].evidence.quote, "costs 30 euro");
    assert_eq!(view.claims[&price.others[0].assertion_id].evidence[0].evidence.quote, "costs 90 euro");
    // Only the current value filters.
    let f = ("price".to_string(), "eq".to_string(), "90".to_string());
    assert!(w.s.list_entities(&w.admin, &w.v, "Widget", &[f], None, 10).unwrap().is_empty());
    // A machine's later claim doesn't displace a person's, and is still shown.
    let o =
        w.s.propose_entity_edit(&w.agent, &w.v, &id, edit("e3", 2, vec![cited("price", json!(12), "12 kg")]), false)
            .unwrap();
    assert_eq!(state(&o), ChangeSetState::Pending, "a machine's claim waits for review");
    w.s.approve(&w.admin, &w.v, &cs_id(&o), None).unwrap();
    let view = w.s.get_entity(&w.admin, &w.v, &id).unwrap();
    let price = view.entity.field("price").unwrap();
    assert_eq!(price.value, json!(30));
    assert!(price.why.contains("newer"), "the claim closest to it is the other person's: {}", price.why);
    assert_eq!(price.others.len(), 2);
    assert!(price.others.iter().any(|o| o.value == json!(12)));
    // Replacing a claim on purpose removes it from the state.
    let chosen = price.assertion_id.clone();
    let version = view.claims[&chosen].assertion.header.version;
    let mut input = field("price", json!(31));
    input.replaces = Some((chosen, version));
    let base = view.entity.header.version;
    w.s.propose_entity_edit(&w.human, &w.v, &id, edit("e4", base, vec![input]), false).unwrap();
    let view = w.s.get_entity(&w.admin, &w.v, &id).unwrap();
    assert_eq!(view.entity.field("price").unwrap().value, json!(31));
    assert_eq!(view.entity.field("price").unwrap().others.len(), 2);
    assert!(!view.entity.field("price").unwrap().others.iter().any(|o| o.value == json!(30)));
}

/// Verification 4.
#[test]
fn additive_versions_keep_old_entities_valid_and_breaking_ones_wait() {
    let w = world();
    assert_eq!(define(&w, &w.human, "Widget", None, widget_v1(), "t1").risk, RiskDecision::AutoCommit);
    let old = widget(&w, "e1", "Old");
    assert_eq!(w.s.get_entity(&w.admin, &w.v, &old).unwrap().entity.type_version, 1);
    let o = define(&w, &w.human, "Widget", Some(1), with_color(), "t2");
    assert_eq!(o.risk, RiskDecision::AutoCommit);
    assert_eq!(state(&o), ChangeSetState::Committed);
    // The old entity reads as it did, and an edit that says nothing of the new field is valid under the new version.
    let edit = EntityEdit {
        base_version: 1,
        aliases: Some(vec!["Veteran".into()]),
        fields: vec![],
        rationale: "r".into(),
        idempotency_key: "e2".into(),
    };
    assert_eq!(
        state(&w.s.propose_entity_edit(&w.human, &w.v, &old, edit, false).unwrap()),
        ChangeSetState::Committed
    );
    let view = w.s.get_entity(&w.admin, &w.v, &old).unwrap();
    assert_eq!(view.entity.type_version, 2);
    assert_eq!(view.entity.field("name").unwrap().value, json!("Old"));
    // New entities can use the new field at once.
    let (_, o) = new_widget(&w, &w.human, "e3", vec![field("name", json!("New")), field("color", json!("red"))]);
    assert_eq!(state(&o), ChangeSetState::Committed);
    // Breaking: a required new field. It waits, and nothing changes until a person approves.
    let mut breaking = with_color();
    breaking["properties"]["weight"] = json!({"type": "number"});
    breaking["required"] = json!(["name", "weight"]);
    let o = define(&w, &w.human, "Widget", Some(2), breaking.clone(), "t3");
    assert_eq!(o.risk, RiskDecision::Review);
    assert_eq!(state(&o), ChangeSetState::Pending);
    let cs = cs_id(&o);
    assert_eq!(w.s.entity_type_history(&w.admin, &w.v, "Widget").unwrap().len(), 2);
    let (_, o) = new_widget(&w, &w.human, "e4", vec![field("name", json!("Still fine"))]);
    assert_eq!(state(&o), ChangeSetState::Committed, "the old rules still hold");
    let approved = w.s.approve(&w.admin, &w.v, &cs, Some("agreed".into())).unwrap();
    assert_eq!(approved.state, ChangeSetState::Committed);
    let history = w.s.entity_type_history(&w.admin, &w.v, "Widget").unwrap();
    assert_eq!(history.iter().map(|h| h.change.as_str()).collect::<Vec<_>>(), ["new", "additive", "breaking"]);
    assert!(history[2].reasons.iter().any(|r| r.contains("weight")));
    // Old entities stay readable at the version they were checked against, and an edit has to satisfy the new rules.
    let view = w.s.get_entity(&w.admin, &w.v, &old).unwrap();
    assert_eq!(view.entity.type_version, 2);
    let edit = |key: &str, fields| EntityEdit {
        base_version: 2,
        aliases: None,
        fields,
        rationale: "r".into(),
        idempotency_key: key.to_string(),
    };
    let o =
        w.s.propose_entity_edit(&w.human, &w.v, &old, edit("e5", vec![field("color", json!("blue"))]), false)
            .unwrap();
    assert_eq!(state(&o), ChangeSetState::Rejected);
    assert!(o.validators.iter().any(|v| !v.passed && v.validator == "entity"), "{:?}", o.validators);
    let o =
        w.s.propose_entity_edit(&w.human, &w.v, &old, edit("e6", vec![field("weight", json!(3))]), false)
            .unwrap();
    assert_eq!(state(&o), ChangeSetState::Committed);
    assert_eq!(w.s.get_entity(&w.admin, &w.v, &old).unwrap().entity.type_version, 3);
}

#[test]
fn a_stale_base_version_on_a_type_is_caught_at_commit_and_approval() {
    let w = world();
    define(&w, &w.human, "Widget", None, widget_v1(), "t1");
    let mut breaking = widget_v1();
    breaking["properties"]["price"] = json!({"type": "string"});
    let waiting = define(&w, &w.human, "Widget", Some(1), breaking, "t2");
    define(&w, &w.human, "Widget", Some(1), with_color(), "t3");
    assert!(matches!(
        w.s.approve(&w.admin, &w.v, &cs_id(&waiting), None),
        Err(ServerError::StaleVersion { .. })
    ));
    let mut newer = with_color();
    newer["properties"]["size"] = json!({"type": "number"});
    assert!(matches!(
        w.s.propose_entity_type(&w.human, &w.v, "Widget", Some(1), newer, "r".into(), "t4".into()),
        Err(ServerError::StaleVersion { .. })
    ));
    assert!(matches!(
        w.s.propose_entity_type(&w.human, &w.v, "Widget", None, with_color(), "r".into(), "t5".into()),
        Ok(o) if state(&o) == ChangeSetState::Rejected
    ));
}

#[test]
fn a_seed_type_is_usable_and_can_get_a_new_version() {
    let w = world();
    let types = w.s.list_entity_types(&w.admin, &w.v, None, 100).unwrap();
    assert_eq!(
        types.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(),
        ["Decision", "Organization", "Person", "Product", "Project"]
    );
    let (id, o) =
        w.s.propose_new_entity(
            &w.human,
            &w.v,
            NewEntity {
                type_name: "Product".into(),
                aliases: vec!["The Pump".into()],
                fields: vec![
                    field("name", json!("Pump")),
                    field("price", json!(99.5)),
                    field("released", json!("2026-01-15")),
                ],
                rationale: "r".into(),
                idempotency_key: "e".into(),
            },
            false,
        )
        .unwrap();
    assert_eq!(state(&o), ChangeSetState::Committed, "{:?}", o.validators);
    let f = |field: &str, op: &str, value: &str| (field.to_string(), op.to_string(), value.to_string());
    for filter in [f("price", "lt", "100"), f("released", "gte", "2026-01-01"), f("name", "eq", "pump")] {
        assert_eq!(
            w.s.list_entities(&w.admin, &w.v, "Product", std::slice::from_ref(&filter), None, 10)
                .unwrap()
                .len(),
            1,
            "{filter:?}"
        );
    }
    assert!(
        w.s.list_entities(&w.admin, &w.v, "Product", &[f("price", "gt", "100")], None, 10)
            .unwrap()
            .is_empty()
    );
    assert_eq!(w.s.list_entities(&w.admin, &w.v, "Product", &[], None, 10).unwrap()[0].header.id, id);
    let mut next = w.s.entity_type_history(&w.admin, &w.v, "Product").unwrap().remove(0).schema;
    next["properties"]["weight"] = json!({"type": "number", "filterable": true});
    assert_eq!(state(&define(&w, &w.human, "Product", Some(1), next, "t")), ChangeSetState::Committed);
    let history = w.s.entity_type_history(&w.admin, &w.v, "Product").unwrap();
    assert_eq!(history.iter().map(|h| h.version_no).collect::<Vec<_>>(), [1, 2]);
    assert_eq!(history[0].change, "seed");
    let listed = w.s.list_entity_types(&w.admin, &w.v, None, 100).unwrap();
    assert_eq!(listed.iter().find(|t| t.name == "Product").unwrap().version_no, 2);
    assert_eq!(listed.len(), 5);
    let page = w.s.list_entity_types(&w.admin, &w.v, Some("Person"), 2).unwrap();
    assert_eq!(page.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), ["Product", "Project"]);
}

/// Verification 5.
#[test]
fn a_merge_and_its_revert_restore_the_state_and_old_ids_redirect_while_merged() {
    let w = world();
    define(&w, &w.human, "Widget", None, widget_v1(), "t");
    let (a, _) = new_widget(
        &w,
        &w.human,
        "e1",
        vec![cited("name", json!("Pump"), "The pump"), cited("price", json!(90), "costs 90 euro")],
    );
    let (b, _) = new_widget(&w, &w.human, "e2", vec![field("name", json!("Pump II")), field("price", json!(30))]);
    let snap_of = |id: &ObjectId| match w
        .s
        .engine()
        .store()
        .snapshot(&w.v)
        .unwrap()
        .get(&RowKey {
            table: Table::Entity,
            id: id.clone(),
        })
        .unwrap()
    {
        Some(Row::Entity(e)) => e,
        other => panic!("{other:?}"),
    };
    let (a_before, b_before) = (snap_of(&a), snap_of(&b));
    project(&w);
    let o = w.s.propose_merge(&w.human, &w.v, &a, &b, "the same pump".into(), "m1".into()).unwrap();
    assert_eq!(state(&o), ChangeSetState::Pending);
    assert_eq!(o.risk, RiskDecision::Review);
    // Nothing changes before approval.
    assert_eq!(snap_of(&a), a_before);
    assert_eq!(w.s.entity_redirect(&w.admin, &w.v, &a).unwrap().current_id, a);
    w.s.approve(&w.admin, &w.v, &cs_id(&o), None).unwrap();
    // Old id redirects, and the target has both sets of claims, with the conflict visible.
    let r = w.s.entity_redirect(&w.admin, &w.v, &a).unwrap();
    assert_eq!(r.current_id, b);
    assert_eq!(r.chain, std::slice::from_ref(&a));
    let view = w.s.get_entity(&w.admin, &w.v, &a).unwrap();
    assert_eq!(view.entity.header.id, b);
    assert_eq!(view.redirected_from, Some(a.clone()));
    assert_eq!(view.entity.field("price").unwrap().others.len(), 1);
    assert_eq!(view.entity.merged_from, std::slice::from_ref(&a));
    // A merged entity is out of lists, filters, values and the index.
    let f = ("name".to_string(), "eq".to_string(), "pump".to_string());
    assert_eq!(w.s.list_entities(&w.admin, &w.v, "Widget", &[], None, 10).unwrap().len(), 1);
    project(&w);
    let hits =
        w.s.search_lexical(
            &w.admin,
            &w.v,
            "Pump",
            LexicalFilter {
                kinds: vec![DocKind::Entity],
                ..LexicalFilter::default()
            },
            10,
        )
        .unwrap();
    assert_eq!(hits.hits.iter().map(|h| h.id.clone()).collect::<Vec<_>>(), std::slice::from_ref(&b));
    // Merging an already merged entity, or into itself, is refused.
    assert!(matches!(w.s.propose_merge(&w.human, &w.v, &b, &b, "r".into(), "m2".into()), Ok(o) if state(&o) == ChangeSetState::Rejected));
    let o2 = w.s.propose_merge(&w.human, &w.v, &a, &b, "r".into(), "m3".into()).unwrap();
    assert_eq!(state(&o2), ChangeSetState::Rejected);
    // Revert.
    let preview = w.s.inverse_preview(&w.admin, &w.v, &cs_id(&o)).unwrap();
    assert_eq!(preview.1, None);
    w.s.revert(&w.admin, &w.v, &cs_id(&o), "not the same".into()).unwrap();
    assert_eq!(snap_of(&a), a_before, "the source is back exactly");
    assert_eq!(snap_of(&b), b_before, "and so is the target");
    assert_eq!(w.s.entity_redirect(&w.admin, &w.v, &a).unwrap().current_id, a, "and the id no longer redirects");
    assert_eq!(w.s.list_entities(&w.admin, &w.v, "Widget", &[f], None, 10).unwrap()[0].header.id, a);
    project(&w);
    let hits =
        w.s.search_lexical(
            &w.admin,
            &w.v,
            "Pump",
            LexicalFilter {
                kinds: vec![DocKind::Entity],
                ..LexicalFilter::default()
            },
            10,
        )
        .unwrap();
    assert_eq!(hits.hits.len(), 2);
}

#[test]
fn merges_chain_and_a_later_change_blocks_a_stale_revert() {
    let w = world();
    define(&w, &w.human, "Widget", None, widget_v1(), "t");
    let (a, b, c) = (widget(&w, "e1", "A"), widget(&w, "e2", "B"), widget(&w, "e3", "C"));
    let merge = |from: &ObjectId, to: &ObjectId, key: &str| {
        let o = w.s.propose_merge(&w.human, &w.v, from, to, "r".into(), key.into()).unwrap();
        w.s.approve(&w.admin, &w.v, &cs_id(&o), None).unwrap();
        cs_id(&o)
    };
    let first = merge(&a, &b, "m1");
    let second = merge(&b, &c, "m2");
    let r = w.s.entity_redirect(&w.admin, &w.v, &a).unwrap();
    assert_eq!(r.current_id, c);
    assert_eq!(r.chain, [a.clone(), b.clone()]);
    let view = w.s.get_entity(&w.admin, &w.v, &a).unwrap();
    assert_eq!(view.entity.header.id, c);
    assert_eq!(view.entity.field("name").unwrap().others.len(), 2, "all three names");
    // The first merge's rows have moved on, so reverting it would lose the second.
    assert!(matches!(w.s.revert(&w.admin, &w.v, &first, "r".into()), Err(ServerError::NotReversible(_))));
    w.s.revert(&w.admin, &w.v, &second, "r".into()).unwrap();
    w.s.revert(&w.admin, &w.v, &first, "r".into()).unwrap();
    assert_eq!(w.s.get_entity(&w.admin, &w.v, &a).unwrap().entity.header.id, a);
    // A claim about an old id lands on the entity it leads to.
    merge(&a, &b, "m3");
    let claim = Op::AddAssertion {
        id: ObjectId("late".into()),
        subject: memex_types::entity_subject(&a),
        predicate: "price".into(),
        value: json!(5),
        epistemic: memex_types::Epistemic::Asserted,
        machine: false,
        confidence_permille: 0,
        valid_from_ms: None,
        valid_to_ms: None,
        evidence: vec![],
    };
    let o =
        w.s.propose(
            &w.human,
            &w.v,
            Proposal {
                ops: vec![claim],
                rationale: "r".into(),
                idempotency_key: "late".into(),
            },
            false,
        )
        .unwrap();
    assert_eq!(state(&o), ChangeSetState::Committed, "{:?}", o.validators);
    assert_eq!(w.s.get_entity(&w.admin, &w.v, &b).unwrap().entity.field("price").unwrap().value, json!(5));
}

#[test]
fn a_split_moves_claims_and_aliases_and_can_be_reverted() {
    let w = world();
    define(&w, &w.human, "Widget", None, widget_v1(), "t");
    let (a, _) =
        w.s.propose_new_entity(
            &w.human,
            &w.v,
            NewEntity {
                type_name: "Widget".into(),
                aliases: vec!["one".into(), "two".into()],
                fields: vec![field("name", json!("Both")), cited("price", json!(90), "costs 90 euro")],
                rationale: "r".into(),
                idempotency_key: "e".into(),
            },
            false,
        )
        .unwrap();
    let before = w.s.get_entity(&w.admin, &w.v, &a).unwrap();
    let price = before.entity.field("price").unwrap().assertion_id.clone();
    let request = SplitRequest {
        base_version: 1,
        moved: vec![price.clone()],
        aliases: vec!["two".into()],
        fields: vec![field("name", json!("Second"))],
        rationale: "two things".into(),
        idempotency_key: "s".into(),
    };
    let (b, o) = w.s.propose_split(&w.human, &w.v, &a, request.clone()).unwrap();
    assert_eq!(state(&o), ChangeSetState::Pending);
    assert!(matches!(w.s.get_entity(&w.admin, &w.v, &b), Err(ServerError::NotFound)));
    let cs = cs_id(&o);
    w.s.approve(&w.admin, &w.v, &cs, None).unwrap();
    let (kept, split) = (w.s.get_entity(&w.admin, &w.v, &a).unwrap(), w.s.get_entity(&w.admin, &w.v, &b).unwrap());
    assert_eq!(kept.entity.aliases, ["one"]);
    assert!(kept.entity.field("price").is_none());
    assert_eq!(split.entity.aliases, ["two"]);
    let moved = split.entity.field("price").unwrap();
    assert_eq!(moved.value, json!(90));
    assert_eq!(
        split.claims[&moved.assertion_id].evidence[0].evidence.quote, "costs 90 euro",
        "the evidence follows"
    );
    assert_eq!(split.entity.field("name").unwrap().value, json!("Second"));
    // The old claim is superseded, not edited.
    let lineage = w.s.lineage(&w.admin, &w.v, &price);
    assert!(lineage.is_ok());
    w.s.revert(&w.admin, &w.v, &cs, "r".into()).unwrap();
    assert_eq!(w.s.get_entity(&w.admin, &w.v, &a).unwrap().entity.field("price").unwrap().assertion_id, price);
    assert!(matches!(w.s.get_entity(&w.admin, &w.v, &b), Err(ServerError::NotFound)));
    // Bad splits.
    let bad = |edit: fn(&mut SplitRequest), key: &str| {
        let mut r = request.clone();
        r.idempotency_key = key.into();
        edit(&mut r);
        let (_, o) = w.s.propose_split(&w.human, &w.v, &a, r).unwrap();
        assert_eq!(state(&o), ChangeSetState::Rejected, "{key}");
    };
    let version = w.s.get_entity(&w.admin, &w.v, &a).unwrap().entity.header.version;
    assert_eq!(version, 1, "a revert puts the row back as it was");
    bad(
        |r| {
            r.aliases = vec!["nope".into()];
        },
        "s2",
    );
    bad(
        |r| {
            r.fields = vec![];
            r.moved = vec![];
            r.aliases = vec![];
        },
        "s3",
    );
    bad(
        |r| {
            r.moved = vec![ObjectId("nope".into())];
        },
        "s4",
    );
    bad(
        |r| {
            r.fields = vec![];
            r.aliases = vec!["two".into()];
            r.moved = vec![];
        },
        "s5",
    );
}

/// Verification 7.
#[test]
fn bad_cases_are_refused_and_nothing_is_written() {
    let w = world();
    let mut bad_schemas = vec![
        ("not a schema", json!({"type": "not-a-type"})),
        (
            "remote ref",
            json!({"type": "object", "additionalProperties": false, "properties": {"a": {"$ref": "https://example.com/x.json"}}}),
        ),
        (
            "file ref",
            json!({"type": "object", "additionalProperties": false, "properties": {"a": {"$ref": "file:///etc/passwd"}}}),
        ),
        ("open object", json!({"type": "object", "properties": {"a": {"type": "string"}}})),
    ];
    let wide: serde_json::Map<String, Value> = (0..10_000).map(|i| (format!("f{i}"), json!({"type": "string"}))).collect();
    bad_schemas.push((
        "10k distinct fields",
        json!({"type": "object", "additionalProperties": false, "properties": wide}),
    ));
    for (i, (name, schema)) in bad_schemas.into_iter().enumerate() {
        let o = define(&w, &w.human, "Bad", None, schema.clone(), &format!("b{i}"));
        assert_eq!(state(&o), ChangeSetState::Rejected, "{name}");
        assert_eq!(o.risk, RiskDecision::Reject, "{name}");
        let failed = o.validators.iter().find(|v| !v.passed).expect(name);
        assert_eq!(failed.validator, "entity", "{name}");
        let preview = w.s.preview_entity_type(&w.human, &w.v, "Bad", &schema).unwrap();
        assert!(preview.preview.problem.is_some(), "{name}");
        assert_eq!(preview.risk, RiskDecision::Reject, "{name}");
    }
    assert!(matches!(w.s.entity_type_history(&w.admin, &w.v, "Bad"), Err(ServerError::NotFound)));
    define(&w, &w.human, "Widget", None, widget_v1(), "t");
    // Values failing validation: the wrong type, an unknown field, a missing required field, and a bad date.
    let bad_values: Vec<Vec<FieldInput>> = vec![
        vec![field("name", json!("x")), field("price", json!("cheap"))],
        vec![field("name", json!("x")), field("nope", json!(1))],
        vec![field("price", json!(1))],
        vec![field("name", json!(5))],
    ];
    for (i, fields) in bad_values.into_iter().enumerate() {
        let (id, o) = new_widget(&w, &w.human, &format!("v{i}"), fields);
        assert_eq!(state(&o), ChangeSetState::Rejected, "case {i}");
        assert!(
            o.validators.iter().any(|v| v.validator == "entity" && !v.passed),
            "case {i}: {:?}",
            o.validators
        );
        assert!(matches!(w.s.get_entity(&w.admin, &w.v, &id), Err(ServerError::NotFound)));
    }
    let (_, o) =
        w.s.propose_new_entity(
            &w.human,
            &w.v,
            NewEntity {
                type_name: "Nope".into(),
                aliases: vec![],
                fields: vec![],
                rationale: "r".into(),
                idempotency_key: "nt".into(),
            },
            false,
        )
        .unwrap();
    assert_eq!(state(&o), ChangeSetState::Rejected);
    // A claim about an entity that doesn't exist.
    let claim = Op::AddAssertion {
        id: ObjectId("orphan".into()),
        subject: "entity:nobody".into(),
        predicate: "name".into(),
        value: json!("x"),
        epistemic: memex_types::Epistemic::Asserted,
        machine: false,
        confidence_permille: 0,
        valid_from_ms: None,
        valid_to_ms: None,
        evidence: vec![],
    };
    let o =
        w.s.propose(
            &w.human,
            &w.v,
            Proposal {
                ops: vec![claim],
                rationale: "r".into(),
                idempotency_key: "o".into(),
            },
            false,
        )
        .unwrap();
    assert_eq!(state(&o), ChangeSetState::Rejected);
    // Too many aliases, and a blank one.
    let aliases: Vec<String> = (0..51).map(|i| format!("a{i}")).collect();
    for (i, aliases) in [aliases, vec![" ".to_string()]].into_iter().enumerate() {
        let (_, o) =
            w.s.propose_new_entity(
                &w.human,
                &w.v,
                NewEntity {
                    type_name: "Widget".into(),
                    aliases,
                    fields: vec![field("name", json!("x"))],
                    rationale: "r".into(),
                    idempotency_key: format!("al{i}"),
                },
                false,
            )
            .unwrap();
        assert_eq!(state(&o), ChangeSetState::Rejected);
    }
}

/// Verification 7, the governance half: an agent can't commit a merge, however much it has earned, and an agent can't approve one.
#[test]
fn a_merge_by_an_agent_never_auto_commits() {
    let w = world();
    define(&w, &w.human, "Widget", None, widget_v1(), "t");
    let (a, b) = (widget(&w, "e1", "A"), widget(&w, "e2", "B"));
    for earned in [false, true] {
        if earned {
            w.s.record_audit_samples(&w.v, &w.agent.principal.id, "", "entity:Widget", 100, 0, "test", "earned for this test")
                .unwrap();
        }
        let o = w.s.propose_merge(&w.agent, &w.v, &a, &b, "r".into(), format!("m{earned}")).unwrap();
        assert_eq!(state(&o), ChangeSetState::Pending, "earned {earned}");
        assert_eq!(o.risk, RiskDecision::Review);
        assert!(o.changeset.as_ref().unwrap().proposer_is_machine);
        // Holding admin scope doesn't make an agent a person.
        assert!(matches!(w.s.approve(&w.agent_admin, &w.v, &cs_id(&o), None), Err(ServerError::Forbidden)));
        assert_eq!(w.s.get_entity(&w.admin, &w.v, &a).unwrap().redirected_from, None);
        w.s.reject(&w.admin, &w.v, &cs_id(&o), "no".into()).unwrap();
    }
    // A merge goes through the generic proposal route the same way.
    let op = Op::MergeEntities {
        source: a.clone(),
        source_version: 1,
        target: b.clone(),
        target_version: 1,
    };
    let o =
        w.s.propose(
            &w.agent,
            &w.v,
            Proposal {
                ops: vec![op],
                rationale: "r".into(),
                idempotency_key: "g".into(),
            },
            false,
        )
        .unwrap();
    assert_eq!(state(&o), ChangeSetState::Pending);
    let approved = w.s.approve(&w.admin, &w.v, &cs_id(&o), None).unwrap();
    assert_eq!(approved.decided_by.as_deref(), Some(w.admin.principal.id.as_str()));
    // An agent can't revert it either.
    assert!(matches!(w.s.revert(&w.agent_admin, &w.v, &cs_id(&o), "r".into()), Err(ServerError::Forbidden)));
    // Nor can it bring a type live, or approve its own.
    let o = define(&w, &w.agent, "AgentType", None, widget_v1(), "at");
    assert_eq!(state(&o), ChangeSetState::Pending);
    let preview = w.s.preview_entity_type(&w.agent, &w.v, "AgentType2", &widget_v1()).unwrap();
    assert_eq!(preview.risk, RiskDecision::Review);
    assert!(matches!(w.s.approve(&w.agent_admin, &w.v, &cs_id(&o), None), Err(ServerError::Forbidden)));
    assert_eq!(w.s.approve(&w.admin, &w.v, &cs_id(&o), None).unwrap().state, ChangeSetState::Committed);
    // While a machine's ordinary claims about entities, once earned, do commit.
    let (_, o) = new_widget(&w, &w.agent, "ae", vec![cited("name", json!("By agent"), "The pump")]);
    assert_eq!(state(&o), ChangeSetState::Committed, "{:?}", o.validators);
    // A machine's claim needs evidence even so.
    let (_, o) = new_widget(&w, &w.agent, "ae2", vec![field("name", json!("No evidence"))]);
    assert_eq!(state(&o), ChangeSetState::Rejected);
}

#[test]
fn a_type_revert_is_refused_while_anything_depends_on_it() {
    let w = world();
    let o1 = define(&w, &w.human, "Widget", None, widget_v1(), "t1");
    let o2 = define(&w, &w.human, "Widget", Some(1), with_color(), "t2");
    assert!(
        matches!(w.s.revert(&w.admin, &w.v, &cs_id(&o1), "r".into()), Err(ServerError::NotReversible(_))),
        "a newer version exists"
    );
    w.s.revert(&w.admin, &w.v, &cs_id(&o2), "r".into()).unwrap();
    assert_eq!(w.s.entity_type_history(&w.admin, &w.v, "Widget").unwrap().len(), 1);
    widget(&w, "e", "X");
    let preview = w.s.inverse_preview(&w.admin, &w.v, &cs_id(&o1)).unwrap();
    assert!(preview.1.is_some());
    assert!(matches!(
        w.s.revert(&w.admin, &w.v, &cs_id(&o1), "r".into()),
        Err(ServerError::NotReversible(_))
    ));
}

#[test]
fn a_new_annotation_reaches_old_entities_without_a_reindex_request() {
    let w = world();
    let mut plain = widget_v1();
    plain["properties"]["notes"] = json!({"type": "string"});
    define(&w, &w.human, "Widget", None, plain.clone(), "t1");
    let (id, _) = new_widget(
        &w,
        &w.human,
        "e",
        vec![field("name", json!("A")), field("notes", json!("quixotic")), field("price", json!(5))],
    );
    project(&w);
    let lexical = |text: &str| {
        w.s.search_lexical(
            &w.admin,
            &w.v,
            text,
            LexicalFilter {
                kinds: vec![DocKind::Entity],
                ..LexicalFilter::default()
            },
            10,
        )
        .unwrap()
        .hits
        .len()
    };
    assert_eq!(lexical("quixotic"), 0);
    let mut searchable = plain.clone();
    searchable["properties"]["notes"]["searchable"] = json!(true);
    searchable["properties"]["notes"]["filterable"] = json!(true);
    assert_eq!(state(&define(&w, &w.human, "Widget", Some(1), searchable, "t2")), ChangeSetState::Committed);
    project(&w);
    assert_eq!(lexical("quixotic"), 1);
    let f = ("notes".to_string(), "eq".to_string(), "QUIXOTIC".to_string());
    assert_eq!(
        w.s.list_entities(&w.admin, &w.v, "Widget", &[f], None, 10).unwrap()[0].header.id,
        id,
        "filters work on the values stored before"
    );
}

#[test]
fn resolve_lists_candidates_exact_first_and_decides_nothing() {
    let w = world();
    define(&w, &w.human, "Widget", None, widget_v1(), "t");
    let (a, _) =
        w.s.propose_new_entity(
            &w.human,
            &w.v,
            NewEntity {
                type_name: "Widget".into(),
                aliases: vec!["Big Pump".into()],
                fields: vec![field("name", json!("Zephyr 9"))],
                rationale: "r".into(),
                idempotency_key: "e".into(),
            },
            false,
        )
        .unwrap();
    let b = widget(&w, "e2", "Big Pump Station");
    let c = widget(&w, "e3", "big pump");
    project(&w);
    let r = w.s.resolve_mention(&w.reader, &w.v, "Big  PUMP", None, 10).unwrap();
    assert_eq!(r.degraded, Vec::<String>::new());
    let got: Vec<(ObjectId, bool)> = r.candidates.iter().map(|c| (c.entity.header.id.clone(), c.exact)).collect();
    assert_eq!(got[..2].iter().filter(|(_, e)| *e).count(), 2);
    assert!(got[..2].iter().any(|(id, _)| *id == a));
    assert!(got[..2].iter().any(|(id, _)| *id == c));
    assert!(got.contains(&(b.clone(), false)), "{got:?}");
    let on: Vec<Option<String>> = r.candidates.iter().filter(|c| c.exact).map(|c| c.matched_on.clone()).collect();
    assert!(on.contains(&Some("$alias".into())) && on.contains(&Some("name".into())), "{on:?}");
    let only = w.s.resolve_mention(&w.reader, &w.v, "Big Pump", Some("Person"), 10).unwrap();
    assert!(only.candidates.is_empty());
    assert!(matches!(w.s.resolve_mention(&w.reader, &w.v, "  ", None, 10), Err(ServerError::Invalid(_))));
    assert_eq!(w.s.resolve_mention(&w.reader, &w.v, "?!", None, 10).unwrap().candidates.len(), 0);
    // Suggestions read the same values, and are never written as anything.
    let head = w.s.engine().store().snapshot(&w.v).unwrap().head().unwrap();
    let same = w.s.same_as_suggestions(&w.reader, &w.v, "Widget", None, 10).unwrap();
    assert_eq!(same.len(), 0, "an alias and a name don't count as the same field: {same:?}");
    let d = widget(&w, "e4", "BIG PUMP");
    let same = w.s.same_as_suggestions(&w.reader, &w.v, "Widget", None, 10).unwrap();
    assert_eq!(same.len(), 1);
    assert_eq!(same[0].text, "big pump");
    assert_eq!(same[0].entity_ids.len(), 2);
    assert!(same[0].entity_ids.contains(&d));
    assert_eq!(
        w.s.engine().store().snapshot(&w.v).unwrap().head().unwrap(),
        head.map(|h| h + 1),
        "only the entity added here"
    );
}

#[test]
fn reads_are_batched_and_do_not_grow_with_the_number_of_entities() {
    let reads = |n: usize| {
        let w = world();
        define(&w, &w.human, "Widget", None, widget_v1(), "t");
        for i in 0..n {
            new_widget(
                &w,
                &w.human,
                &format!("e{i}"),
                vec![cited("name", json!(format!("Unique{i} pump")), "The pump"), field("price", json!(i))],
            );
        }
        project(&w);
        let store = w.s.engine().store();
        let before = store.counts().reads.load(std::sync::atomic::Ordering::Relaxed);
        let mut r = SearchRequest::new("pump");
        r.mode = SearchMode::Lexical;
        r.filter.kinds = vec![DocKind::Entity];
        r.limit = 100;
        let p = w.s.search(&w.admin, &w.v, &r).unwrap();
        assert_eq!(p.items.iter().filter(|i| i.kind == ItemKind::Entity).count(), n);
        let list =
            w.s.list_entities(&w.admin, &w.v, "Widget", &[("price".into(), "gte".into(), "0".into())], None, 100)
                .unwrap();
        assert_eq!(list.len(), n);
        store.counts().reads.load(std::sync::atomic::Ordering::Relaxed) - before
    };
    assert_eq!(reads(2), reads(20));
}

#[test]
fn a_past_state_includes_the_entities_of_that_time() {
    let w = world();
    define(&w, &w.human, "Widget", None, widget_v1(), "t");
    let id = widget(&w, "e", "Quixotic");
    let head = w.s.engine().store().snapshot(&w.v).unwrap().head().unwrap().unwrap();
    let o =
        w.s.propose_entity_edit(
            &w.human,
            &w.v,
            &id,
            EntityEdit {
                base_version: 1,
                aliases: Some(vec!["Later".into()]),
                fields: vec![],
                rationale: "r".into(),
                idempotency_key: "e2".into(),
            },
            false,
        )
        .unwrap();
    assert_eq!(state(&o), ChangeSetState::Committed);
    let mut r = SearchRequest::new("Quixotic");
    r.as_of = Some(memex_types::AsOf::Seq(head));
    let p = w.s.search(&w.admin, &w.v, &r).unwrap();
    let item = p.items.iter().find(|i| i.kind == ItemKind::Entity).expect("the entity then");
    assert!(!item.text.contains("Later"), "{}", item.text);
    let mut r = SearchRequest::new("Later");
    r.as_of = Some(memex_types::AsOf::Seq(head));
    assert!(w.s.search(&w.admin, &w.v, &r).unwrap().items.iter().all(|i| i.kind != ItemKind::Entity));
}

#[test]
fn replaying_the_log_gives_the_same_state() {
    let w = world();
    define(&w, &w.human, "Widget", None, widget_v1(), "t");
    let (a, b) = (widget(&w, "e1", "A"), widget(&w, "e2", "B"));
    let o = w.s.propose_merge(&w.human, &w.v, &a, &b, "r".into(), "m".into()).unwrap();
    w.s.approve(&w.admin, &w.v, &cs_id(&o), None).unwrap();
    let store = w.s.engine().store();
    let snap = store.snapshot(&w.v).unwrap();
    let copy = MemStore::new();
    let lease = copy.open_vault(&w.v).unwrap();
    memex_core::replay(memex_core::all_events(&snap).unwrap(), &copy, &lease).unwrap();
    let dump = memex_core::dump_store(&snap).unwrap();
    assert_eq!(dump, memex_core::dump_store(&copy.snapshot(&w.v).unwrap()).unwrap());
}

#[test]
fn moving_a_claim_away_from_an_entity_updates_that_entity() {
    let w = world();
    define(&w, &w.human, "Widget", None, widget_v1(), "t");
    let (a, _) = new_widget(&w, &w.human, "e", vec![field("name", json!("A")), field("price", json!(5))]);
    let claim = w.s.get_entity(&w.admin, &w.v, &a).unwrap().entity.field("price").unwrap().assertion_id.clone();
    let replacement = Op::AddAssertion {
        id: ObjectId("plain".into()),
        subject: "a free-standing subject".into(),
        predicate: "price".into(),
        value: json!(5),
        epistemic: memex_types::Epistemic::Asserted,
        machine: false,
        confidence_permille: 0,
        valid_from_ms: None,
        valid_to_ms: None,
        evidence: vec![],
    };
    let op = Op::SupersedeAssertion {
        target: claim,
        base_version: 1,
        replacement: Box::new(replacement),
    };
    let o =
        w.s.propose(
            &w.human,
            &w.v,
            Proposal {
                ops: vec![op],
                rationale: "r".into(),
                idempotency_key: "m".into(),
            },
            false,
        )
        .unwrap();
    assert_eq!(state(&o), ChangeSetState::Committed, "{:?}", o.validators);
    assert!(w.s.get_entity(&w.admin, &w.v, &a).unwrap().entity.field("price").is_none());
}

#[test]
fn a_past_state_refuses_a_field_scope_it_could_not_honour() {
    let w = world();
    define(&w, &w.human, "Widget", None, widget_v1(), "t");
    let mut r = SearchRequest::new("x");
    r.as_of = Some(memex_types::AsOf::Seq(0));
    r.filter.attr = Some(memex_types::AttrScope {
        type_name: "Widget".into(),
        field: "name".into(),
    });
    assert!(matches!(w.s.search(&w.admin, &w.v, &r), Err(ServerError::Invalid(_))));
}

#[test]
fn a_retried_creation_gets_the_first_id() {
    let w = world();
    define(&w, &w.human, "Widget", None, widget_v1(), "t");
    let (first, o) = new_widget(&w, &w.human, "same", vec![field("name", json!("A"))]);
    assert!(!o.replayed);
    let (again, o) = new_widget(&w, &w.human, "same", vec![field("name", json!("A"))]);
    assert!(o.replayed);
    assert_eq!(first, again);
    assert_eq!(w.s.list_entities(&w.admin, &w.v, "Widget", &[], None, 10).unwrap().len(), 1);
}

#[test]
fn a_dry_run_writes_nothing() {
    let w = world();
    define(&w, &w.human, "Widget", None, widget_v1(), "t");
    let head = w.s.engine().store().snapshot(&w.v).unwrap().head().unwrap();
    let (_, o) =
        w.s.propose_new_entity(
            &w.human,
            &w.v,
            NewEntity {
                type_name: "Widget".into(),
                aliases: vec![],
                fields: vec![field("name", json!("A"))],
                rationale: "r".into(),
                idempotency_key: "d".into(),
            },
            true,
        )
        .unwrap();
    assert!(o.dry_run);
    assert_eq!(o.risk, RiskDecision::AutoCommit);
    assert_eq!(w.s.engine().store().snapshot(&w.v).unwrap().head().unwrap(), head);
}

#[test]
fn readers_can_read_and_not_write() {
    let w = world();
    define(&w, &w.human, "Widget", None, widget_v1(), "t");
    let id = widget(&w, "e", "A");
    w.s.get_entity(&w.reader, &w.v, &id).unwrap();
    assert!(matches!(
        w.s.propose_entity_type(&w.reader, &w.v, "T", None, widget_v1(), "r".into(), "k".into()),
        Err(ServerError::Forbidden)
    ));
    assert!(matches!(
        w.s.preview_entity_type(&w.reader, &w.v, "T", &widget_v1()),
        Err(ServerError::Forbidden)
    ));
    assert!(matches!(
        w.s.propose_merge(&w.reader, &w.v, &id, &id, "r".into(), "k".into()),
        Err(ServerError::Forbidden)
    ));
}
