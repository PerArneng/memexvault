//! The proposal flow through the facade, in memory.
use memex_core::testing::{ManualClock, MemBlobStore, MemRegistry, MemStore, SeededRandom};
use memex_core::{Caller, NewToken, Proposal, Server, ServerEdges, ServerError, Snapshot, Store, dump_store};
use memex_types::{
    ChangeSetState, Epistemic, EvidenceInput, Grant, ObjectId, Op, PrincipalKind, RiskDecision, Row, RowKey, Scope, Stance, Status, Table, VaultId,
};
use serde_json::{Value, json};
use std::collections::HashSet;
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
    type Caches = memex_core::NoCaches;
}

struct World {
    s: Arc<Server<Fakes>>,
    admin: Caller,
    human: Caller,
    agent: Caller,
    reader: Caller,
    v: VaultId,
}

fn world() -> World {
    let s = Arc::new(Server::<Fakes>::new(
        MemRegistry::new(),
        MemStore::new(),
        MemBlobStore::new(ManualClock::starting_at(0)),
        ManualClock::starting_at(1000),
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
    let (human, agent, reader) = (
        make("human", PrincipalKind::Human, Scope::Propose),
        make("agent", PrincipalKind::Agent, Scope::Propose),
        make("reader", PrincipalKind::Human, Scope::Read),
    );
    s.record_segment(
        &admin,
        &v,
        &ObjectId("seg1".into()),
        &ObjectId("src1".into()),
        Some("sv"),
        "Staden ligger vid Göta älv. Pumpen väger 12 kg.",
    )
    .unwrap();
    World {
        s,
        admin,
        human,
        agent,
        reader,
        v,
    }
}

fn proposal(key: &str, ops: Vec<Op>) -> Proposal {
    Proposal {
        ops,
        rationale: "because".into(),
        idempotency_key: key.into(),
    }
}

fn id(s: &str) -> ObjectId {
    ObjectId(s.into())
}

fn claim(aid: &str, subject: &str, value: Value, evidence: Vec<(&str, &str)>) -> Op {
    Op::AddAssertion {
        id: id(aid),
        subject: subject.into(),
        predicate: "weighs".into(),
        value,
        epistemic: Epistemic::Asserted,
        machine: false,
        confidence_permille: 1000,
        valid_from_ms: None,
        valid_to_ms: None,
        evidence: evidence
            .into_iter()
            .enumerate()
            .map(|(i, (seg, quote))| {
                (
                    id(&format!("{aid}-e{i}")),
                    EvidenceInput {
                        segment_id: id(seg),
                        quote: quote.into(),
                        stance: Stance::Supports,
                    },
                )
            })
            .collect(),
    }
}

fn hash(w: &World) -> String {
    dump_store(&w.s.engine().store().snapshot(&w.v).unwrap()).unwrap().sha256
}

fn page_key(p: &str) -> RowKey {
    RowKey { table: Table::Page, id: id(p) }
}

fn create_page(p: &str) -> Op {
    Op::CreatePage {
        slug: None,
        redirects: vec![],
        id: id(p),
        title: format!("Title {p}"),
        body: "B".into(),
    }
}

#[test]
fn the_same_key_100_times_concurrently_commits_once_and_gives_the_same_result() {
    let w = world();
    let before = w.s.engine().store().snapshot(&w.v).unwrap().head().unwrap().unwrap();
    let threads: Vec<_> = (0..100)
        .map(|_| {
            let (s, c, v) = (w.s.clone(), w.human.clone(), w.v.clone());
            std::thread::spawn(move || s.propose(&c, &v, proposal("once", vec![create_page("p")]), false).unwrap())
        })
        .collect();
    let outcomes: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert_eq!(outcomes.iter().filter(|o| !o.replayed).count(), 1);
    let ids: HashSet<_> = outcomes.iter().map(|o| o.changeset.as_ref().unwrap().header.id.clone()).collect();
    assert_eq!(ids.len(), 1);
    assert!(outcomes.iter().all(|o| o.risk == RiskDecision::AutoCommit));
    assert_eq!(w.s.engine().store().snapshot(&w.v).unwrap().head().unwrap().unwrap(), before + 1);
}

#[test]
fn deletes_always_go_to_review_even_from_a_server_admin() {
    let w = world();
    w.s.propose(&w.admin, &w.v, proposal("a", vec![create_page("p")]), false).unwrap();
    let out =
        w.s.propose(&w.admin, &w.v, proposal("b", vec![Op::DeletePage { id: id("p"), base_version: 1 }]), false)
            .unwrap();
    assert_eq!(out.risk, RiskDecision::Review);
    assert!(w.s.engine().store().snapshot(&w.v).unwrap().get(&page_key("p")).unwrap().is_some());
}

#[test]
fn editing_a_verified_claim_goes_to_review() {
    let w = world();
    let mut verified = claim("a1", "pump", json!(12), vec![]);
    if let Op::AddAssertion { epistemic, .. } = &mut verified {
        *epistemic = Epistemic::Verified;
    }
    w.s.propose(&w.human, &w.v, proposal("a", vec![verified]), false).unwrap();
    let edit = Op::SupersedeAssertion {
        target: id("a1"),
        base_version: 1,
        replacement: Box::new(claim("a2", "pump", json!(13), vec![])),
    };
    assert_eq!(
        w.s.propose(&w.human, &w.v, proposal("b", vec![edit]), false).unwrap().risk,
        RiskDecision::Review
    );
}

#[test]
fn machine_proposals_wait_for_review_until_auto_commit_is_earned_whatever_they_claim() {
    let w = world();
    let good = || claim("a1", "pump", json!(12), vec![("seg1", "Pumpen väger 12 kg")]);
    let out = w.s.propose(&w.agent, &w.v, proposal("a", vec![good()]), false).unwrap();
    assert_eq!(out.risk, RiskDecision::Review);
    assert_eq!(out.changeset.unwrap().state, ChangeSetState::Pending);
    let snap = w.s.engine().store().snapshot(&w.v).unwrap();
    assert_eq!(
        snap.get(&RowKey {
            table: Table::Assertion,
            id: id("a1")
        })
        .unwrap(),
        None
    );
    drop(snap);
    w.s.record_audit_samples(&w.v, &w.agent.principal.id, "", "claim", 100, 0, "test", "earned for this test")
        .unwrap();
    let req = memex_core::UploadRequest {
        name: "t.txt".into(),
        trust_tier: "trusted".into(),
        metadata: json!({}),
        replaces: None,
    };
    let trusted =
        w.s.upload(&w.admin, &w.v, req, memex_core::UploadLimits::default(), &mut "a trusted document".as_bytes())
            .unwrap()
            .source
            .header
            .id;
    w.s.record_segment(&w.admin, &w.v, &id("seg2"), &trusted, None, "Staden ligger vid Göta älv.")
        .unwrap();
    let out =
        w.s.propose(
            &w.agent,
            &w.v,
            proposal("b", vec![claim("a3", "other", json!(1), vec![("seg2", "Göta älv")])]),
            false,
        )
        .unwrap();
    assert_eq!(out.risk, RiskDecision::AutoCommit);
}

#[test]
fn an_unsourced_machine_claim_can_not_become_active_even_if_the_request_says_it_is_not_a_machine() {
    let w = world();
    w.s.record_audit_samples(&w.v, &w.agent.principal.id, "", "claim", 100, 0, "test", "earned for this test")
        .unwrap();
    // The op says machine: false, and the token says agent. The token wins.
    let out =
        w.s.propose(&w.agent, &w.v, proposal("a", vec![claim("a1", "pump", json!(12), vec![])]), false)
            .unwrap();
    assert_eq!(out.risk, RiskDecision::Reject);
    let cs = out.changeset.unwrap();
    assert_eq!(cs.state, ChangeSetState::Rejected);
    assert!(cs.decision_reason.unwrap().contains("no evidence"));
    assert_eq!(
        w.s.engine()
            .store()
            .snapshot(&w.v)
            .unwrap()
            .get(&RowKey {
                table: Table::Assertion,
                id: id("a1")
            })
            .unwrap(),
        None
    );
}

#[test]
fn a_quote_that_is_not_in_the_segment_is_rejected() {
    let w = world();
    let out =
        w.s.propose(
            &w.agent,
            &w.v,
            proposal("a", vec![claim("a1", "pump", json!(12), vec![("seg1", "Pumpen väger 99 kg")])]),
            false,
        )
        .unwrap();
    assert_eq!(out.risk, RiskDecision::Reject);
    assert!(out.validators.iter().any(|v| v.validator == "quote" && !v.passed));
    let out =
        w.s.propose(
            &w.agent,
            &w.v,
            proposal("b", vec![claim("a2", "pump", json!(12), vec![("no-such-seg", "x")])]),
            false,
        )
        .unwrap();
    assert!(out.validators.iter().any(|v| v.validator == "quote" && !v.passed));
}

#[test]
fn a_duplicate_claim_is_rejected_and_a_contradictory_one_coexists_with_a_link() {
    let w = world();
    w.s.propose(&w.human, &w.v, proposal("a", vec![claim("a1", "pump", json!(12), vec![])]), false)
        .unwrap();
    let dup =
        w.s.propose(&w.human, &w.v, proposal("b", vec![claim("a2", "PUMP", json!(12), vec![])]), false)
            .unwrap();
    assert_eq!(dup.risk, RiskDecision::Reject);
    assert!(dup.validators.iter().any(|v| v.validator == "dedup" && !v.passed));
    let other =
        w.s.propose(&w.human, &w.v, proposal("c", vec![claim("a3", "pump", json!(13), vec![])]), false)
            .unwrap();
    assert_eq!(other.risk, RiskDecision::AutoCommit);
    assert_eq!(other.contradictions, vec![(id("a3"), id("a1"))]);
    let snap = w.s.engine().store().snapshot(&w.v).unwrap();
    for a in ["a1", "a3"] {
        let Some(Row::Assertion(row)) = snap
            .get(&RowKey {
                table: Table::Assertion,
                id: id(a),
            })
            .unwrap()
        else {
            panic!("{a}")
        };
        assert_eq!(row.header.status, Status::Active);
    }
    drop(snap);
    assert_eq!(w.s.open_contradictions(&w.reader, &w.v).unwrap(), vec![(id("a3"), id("a1"))]);
}

#[test]
fn a_stale_base_version_returns_the_current_row() {
    let w = world();
    w.s.propose(&w.human, &w.v, proposal("a", vec![create_page("p")]), false).unwrap();
    let edit = |key: &str, base| {
        proposal(
            key,
            vec![Op::EditPage {
                id: id("p"),
                base_version: base,
                body: "x".into(),
            }],
        )
    };
    w.s.propose(&w.human, &w.v, edit("b", 1), false).unwrap();
    let before = hash(&w);
    match w.s.propose(&w.human, &w.v, edit("c", 1), false) {
        Err(ServerError::StaleVersion { base: 1, current, .. }) => assert_eq!(current.header().version, 2),
        other => panic!("{other:?}"),
    }
    assert_eq!(hash(&w), before);
}

#[test]
fn a_change_that_went_stale_during_review_is_refused_at_approval() {
    let w = world();
    w.s.propose(&w.human, &w.v, proposal("a", vec![create_page("p")]), false).unwrap();
    let del =
        w.s.propose(&w.human, &w.v, proposal("b", vec![Op::DeletePage { id: id("p"), base_version: 1 }]), false)
            .unwrap()
            .changeset
            .unwrap();
    w.s.propose(
        &w.human,
        &w.v,
        proposal(
            "c",
            vec![Op::EditPage {
                id: id("p"),
                base_version: 1,
                body: "x".into(),
            }],
        ),
        false,
    )
    .unwrap();
    assert!(matches!(
        w.s.approve(&w.admin, &w.v, &del.header.id, None),
        Err(ServerError::StaleVersion { .. })
    ));
    assert!(w.s.engine().store().snapshot(&w.v).unwrap().get(&page_key("p")).unwrap().is_some());
}

#[test]
fn review_records_who_decided_and_only_admins_can() {
    let w = world();
    let cs =
        w.s.propose(
            &w.agent,
            &w.v,
            proposal("a", vec![claim("a1", "pump", json!(12), vec![("seg1", "Pumpen väger 12 kg")])]),
            false,
        )
        .unwrap()
        .changeset
        .unwrap();
    for c in [&w.reader, &w.human, &w.agent] {
        assert!(matches!(w.s.approve(c, &w.v, &cs.header.id, None), Err(ServerError::Forbidden)));
        assert!(matches!(w.s.reject(c, &w.v, &cs.header.id, "no".into()), Err(ServerError::Forbidden)));
    }
    let queue = w.s.list_changesets(&w.reader, &w.v, Some(ChangeSetState::Pending), None, 10).unwrap();
    assert_eq!(queue.len(), 1);
    let done = w.s.approve(&w.admin, &w.v, &cs.header.id, Some("looks right".into())).unwrap();
    assert_eq!(
        (done.state, done.decided_by.as_deref(), done.decision_reason.as_deref()),
        (ChangeSetState::Committed, Some(w.admin.principal.id.as_str()), Some("looks right"))
    );
    assert!(
        w.s.list_changesets(&w.reader, &w.v, Some(ChangeSetState::Pending), None, 10)
            .unwrap()
            .is_empty()
    );
    // The same person asking again gets the same result, and nothing new happens.
    let head = w.s.read_events(&w.admin, &w.v, None).unwrap().len();
    assert_eq!(w.s.approve(&w.admin, &w.v, &cs.header.id, None).unwrap(), done);
    assert_eq!(w.s.read_events(&w.admin, &w.v, None).unwrap().len(), head);
    let cs2 =
        w.s.propose(
            &w.agent,
            &w.v,
            proposal("b", vec![claim("a2", "other", json!(1), vec![("seg1", "Göta älv")])]),
            false,
        )
        .unwrap()
        .changeset
        .unwrap();
    assert!(matches!(w.s.reject(&w.admin, &w.v, &cs2.header.id, " ".into()), Err(ServerError::Invalid(_))));
    let rejected = w.s.reject(&w.admin, &w.v, &cs2.header.id, "wrong".into()).unwrap();
    assert_eq!(rejected.state, ChangeSetState::Rejected);
    assert_eq!(
        w.s.engine()
            .store()
            .snapshot(&w.v)
            .unwrap()
            .get(&RowKey {
                table: Table::Assertion,
                id: id("a2")
            })
            .unwrap(),
        None
    );
}

#[test]
fn a_dry_run_writes_nothing_and_matches_a_real_submission() {
    let w = world();
    for (i, ops) in [
        vec![create_page("p")],
        vec![claim("a1", "pump", json!(1), vec![("seg1", "not there")])],
        vec![claim("a2", "pump", json!(12), vec![("seg1", "Pumpen väger 12 kg")])],
    ]
    .into_iter()
    .enumerate()
    {
        for caller in [&w.human, &w.agent] {
            let key = format!("k{i}{}", caller.principal.id);
            let before = hash(&w);
            let dry = w.s.propose(caller, &w.v, proposal(&key, ops.clone()), true).unwrap();
            assert_eq!(hash(&w), before, "a dry run must write nothing");
            assert!(dry.dry_run && dry.changeset.is_none());
            let real = w.s.propose(caller, &w.v, proposal(&key, ops.clone()), false).unwrap();
            assert_eq!(
                (dry.validators, dry.risk, dry.contradictions),
                (real.validators, real.risk, real.contradictions)
            );
            // Later real submissions of the same claims collide, so each case uses fresh state.
            if real.changeset.as_ref().is_some_and(|c| c.state == ChangeSetState::Committed) {
                w.s.revert(&w.admin, &w.v, &real.changeset.unwrap().header.id, "reset".into()).unwrap();
            }
        }
    }
}

#[test]
fn as_of_returns_what_was_believed_then_even_after_a_revert() {
    let w = world();
    let head = |w: &World| w.s.engine().store().snapshot(&w.v).unwrap().head().unwrap().unwrap();
    let c1 =
        w.s.propose(&w.human, &w.v, proposal("a", vec![claim("a1", "pump", json!(12), vec![])]), false)
            .unwrap()
            .changeset
            .unwrap();
    let at_first = head(&w);
    let edit = Op::SupersedeAssertion {
        target: id("a1"),
        base_version: 1,
        replacement: Box::new(claim("a2", "pump", json!(13), vec![])),
    };
    let c2 = w.s.propose(&w.human, &w.v, proposal("b", vec![edit]), false).unwrap().changeset.unwrap();
    let at_second = head(&w);
    w.s.revert(&w.admin, &w.v, &c2.header.id, "wrong".into()).unwrap();
    let after_revert = head(&w);
    let ids = |seq| {
        w.s.believed_at(&w.reader, &w.v, "pump", seq)
            .unwrap()
            .into_iter()
            .map(|a| a.header.id.0)
            .collect::<Vec<_>>()
    };
    assert_eq!(ids(at_first), vec!["a1"]);
    assert_eq!(ids(at_second), vec!["a2"], "the revert must not rewrite the past");
    assert_eq!(ids(after_revert), vec!["a1"]);
    assert!(ids(0).is_empty());
    let _ = c1;
}

#[test]
fn a_revert_is_refused_when_later_work_would_be_lost_or_it_was_already_done() {
    let w = world();
    let c =
        w.s.propose(&w.human, &w.v, proposal("a", vec![create_page("p")]), false)
            .unwrap()
            .changeset
            .unwrap();
    w.s.propose(
        &w.human,
        &w.v,
        proposal(
            "b",
            vec![Op::EditPage {
                id: id("p"),
                base_version: 1,
                body: "later".into(),
            }],
        ),
        false,
    )
    .unwrap();
    let before = hash(&w);
    assert!(matches!(
        w.s.revert(&w.admin, &w.v, &c.header.id, "x".into()),
        Err(ServerError::NotReversible(_))
    ));
    assert_eq!(hash(&w), before);
    let (_, blocked) = w.s.inverse_preview(&w.reader, &w.v, &c.header.id).unwrap();
    assert!(blocked.unwrap().contains("changed since"));
    let d =
        w.s.propose(&w.human, &w.v, proposal("c", vec![create_page("q")]), false)
            .unwrap()
            .changeset
            .unwrap();
    assert_eq!(w.s.inverse_preview(&w.reader, &w.v, &d.header.id).unwrap().1, None);
    w.s.revert(&w.admin, &w.v, &d.header.id, "x".into()).unwrap();
    assert!(matches!(
        w.s.revert(&w.admin, &w.v, &d.header.id, "x".into()),
        Err(ServerError::NotReversible(_))
    ));
    let pending =
        w.s.propose(
            &w.agent,
            &w.v,
            proposal("d", vec![claim("a1", "s", json!(1), vec![("seg1", "Göta älv")])]),
            false,
        )
        .unwrap()
        .changeset
        .unwrap();
    assert!(matches!(
        w.s.revert(&w.admin, &w.v, &pending.header.id, "x".into()),
        Err(ServerError::NotReversible(_))
    ));
}

/// Materialized state without ChangeSet rows, which record the history itself and rightly change.
fn state_without_changesets(w: &World) -> Vec<Row> {
    let snap = w.s.engine().store().snapshot(&w.v).unwrap();
    let mut rows = vec![];
    for table in [Table::Source, Table::Page, Table::Edge, Table::Segment, Table::Assertion, Table::Evidence] {
        let mut after = None;
        loop {
            let page = snap.rows(table, after.as_ref()).unwrap();
            let Some(last) = page.last() else { break };
            after = Some(last.header().id.clone());
            rows.extend(page);
        }
    }
    rows
}

#[test]
fn a_random_changeset_and_its_revert_leave_the_state_unchanged_over_10k_runs() {
    let w = world();
    // A small existing world for the ChangeSets to act on.
    for i in 0..5 {
        w.s.propose(&w.human, &w.v, proposal(&format!("seed-p{i}"), vec![create_page(&format!("base-p{i}"))]), false)
            .unwrap();
        w.s.propose(
            &w.human,
            &w.v,
            proposal(&format!("seed-a{i}"), vec![claim(&format!("base-a{i}"), &format!("s{i}"), json!(i), vec![])]),
            false,
        )
        .unwrap();
    }
    let mut state: u64 = 42;
    let mut rand = move |n: u64| {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state % n
    };
    let current_version = |w: &World, key: RowKey| {
        w.s.engine()
            .store()
            .snapshot(&w.v)
            .unwrap()
            .get(&key)
            .unwrap()
            .map_or(1, |r| r.header().version)
    };
    let (mut reverted, mut pages_edited, mut claims_replaced) = (0, 0, 0);
    for run in 0..10_000u64 {
        let i = rand(5);
        let mut ops = vec![];
        for _ in 0..1 + rand(3) {
            match rand(5) {
                0 => ops.push(create_page(&format!("new-p{run}-{}", ops.len()))),
                1 => {
                    let k = page_key(&format!("base-p{i}"));
                    if !ops.iter().any(|o| matches!(o, Op::EditPage { .. })) {
                        ops.push(Op::EditPage {
                            id: k.id.clone(),
                            base_version: current_version(&w, k),
                            body: format!("edit {run}"),
                        });
                        pages_edited += 1;
                    }
                }
                2 => {
                    let k = RowKey {
                        table: Table::Assertion,
                        id: id(&format!("base-a{i}")),
                    };
                    if !ops.iter().any(|o| matches!(o, Op::SupersedeAssertion { .. })) {
                        ops.push(Op::SupersedeAssertion {
                            target: k.id.clone(),
                            base_version: current_version(&w, k),
                            replacement: Box::new(claim(&format!("rep-{run}"), &format!("s{i}"), json!(1000 + run), vec![])),
                        });
                        claims_replaced += 1;
                    }
                }
                3 => ops.push(claim(
                    &format!("new-a{run}-{}", ops.len()),
                    &format!("s{i}"),
                    json!(run * 2 + ops.len() as u64),
                    vec![],
                )),
                _ => ops.push(Op::AddEdge {
                    id: id(&format!("e{run}-{}", ops.len())),
                    from: id(&format!("base-p{i}")),
                    to: id("x"),
                    kind: "cites".into(),
                }),
            }
        }
        let before = state_without_changesets(&w);
        let out = w.s.propose(&w.human, &w.v, proposal(&format!("run-{run}"), ops), false).unwrap();
        let cs = out.changeset.unwrap();
        assert_eq!(cs.state, ChangeSetState::Committed, "run {run}: {:?}", cs.validators);
        assert_ne!(state_without_changesets(&w), before, "run {run} changed nothing");
        w.s.revert(&w.admin, &w.v, &cs.header.id, "undo".into()).unwrap();
        assert_eq!(state_without_changesets(&w), before, "run {run}");
        reverted += 1;
    }
    assert_eq!(reverted, 10_000);
    assert!(pages_edited > 1000 && claims_replaced > 1000, "{pages_edited} {claims_replaced}");
}

#[test]
fn proposing_needs_propose_scope_and_other_vaults_stay_hidden() {
    let w = world();
    assert!(matches!(
        w.s.propose(&w.reader, &w.v, proposal("a", vec![create_page("p")]), false),
        Err(ServerError::Forbidden)
    ));
    let other = w.s.create_vault(&w.admin, "other").unwrap().id;
    assert!(matches!(
        w.s.propose(&w.human, &other, proposal("a", vec![create_page("p")]), false),
        Err(ServerError::NotFound)
    ));
    assert!(matches!(w.s.list_changesets(&w.human, &other, None, None, 10), Err(ServerError::NotFound)));
}

#[test]
fn a_changeset_is_all_or_nothing_for_readers() {
    let w = world();
    let bad_second = vec![
        create_page("ok"),
        Op::EditPage {
            id: id("missing"),
            base_version: 1,
            body: "x".into(),
        },
    ];
    assert!(w.s.propose(&w.human, &w.v, proposal("a", bad_second), false).is_err());
    assert!(w.s.engine().store().snapshot(&w.v).unwrap().get(&page_key("ok")).unwrap().is_none());
}

#[test]
fn the_log_replays_governance_results_with_no_validators_or_llm() {
    let w = world();
    w.s.propose(
        &w.human,
        &w.v,
        proposal("a", vec![create_page("p"), claim("a1", "pump", json!(12), vec![])]),
        false,
    )
    .unwrap();
    let c =
        w.s.propose(&w.human, &w.v, proposal("b", vec![create_page("q")]), false)
            .unwrap()
            .changeset
            .unwrap();
    w.s.revert(&w.admin, &w.v, &c.header.id, "undo".into()).unwrap();
    let snap = w.s.engine().store().snapshot(&w.v).unwrap();
    let events = memex_core::all_events(&snap).unwrap();
    let source = dump_store(&snap).unwrap().sha256;
    drop(snap);
    let target = MemStore::new();
    let lease = target.open_vault(&w.v).unwrap();
    memex_core::replay(events, &target, &lease).unwrap();
    assert_eq!(dump_store(&target.snapshot(&w.v).unwrap()).unwrap().sha256, source);
}
