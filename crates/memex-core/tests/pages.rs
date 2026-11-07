//! Wiki pages through the facade, in memory.
use memex_core::search::expand::{MAX_ADDED, NEIGHBOURS_PER_SEED, expand};
use memex_core::search::fusion::Fused;
use memex_core::testing::{ManualClock, MemBlobStore, MemRegistry, MemStore, SeededRandom};
use memex_core::{
    Caller, ClaimRefStatus, NewPage, NewToken, Proposal, ProposalOutcome, Server, ServerEdges, ServerError, Snapshot, StepOutcome, Store, dump_store,
};
use memex_types::{
    ChangeSetState, DocKind, Epistemic, EvidenceInput, Expand, Grant, ItemKind, LexicalFilter, ObjectId, Op, PrincipalKind, RiskDecision, Row, RowKey, Scope,
    SearchMode, SearchRequest, Stance, Status, Table, VaultId, WIKI_LINK, slug_target,
};
use serde_json::json;
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
    let (human, agent, reader) = (
        make("human", PrincipalKind::Human, Scope::Propose),
        make("agent", PrincipalKind::Agent, Scope::Propose),
        make("reader", PrincipalKind::Human, Scope::Read),
    );
    World {
        s,
        admin,
        human,
        agent,
        reader,
        v,
    }
}

impl World {
    fn create(&self, title: &str, body: &str) -> ObjectId {
        let new = NewPage {
            title: title.into(),
            slug: None,
            body: body.into(),
            rationale: "r".into(),
            idempotency_key: format!("create-{title}"),
        };
        let (id, out) = self.s.propose_new_page(&self.human, &self.v, new, false).unwrap();
        assert_eq!(out.risk, RiskDecision::AutoCommit, "{:?}", out.validators);
        id
    }

    fn edit(&self, id: &ObjectId, base: u32, body: &str) -> Result<ProposalOutcome, ServerError> {
        self.s.propose_page_edit(
            &self.human,
            &self.v,
            id,
            base,
            body.into(),
            "r".into(),
            format!("edit-{id}-{base}-{body}"),
            false,
        )
    }

    fn rename(&self, id: &ObjectId, base: u32, title: &str) -> Result<ProposalOutcome, ServerError> {
        self.s
            .propose_page_rename(&self.human, &self.v, id, base, title.into(), None, "r".into(), format!("rename-{id}-{base}"))
    }

    fn settle(&self) {
        for _ in 0..50 {
            let a = self.s.project_lexical(&self.v).unwrap();
            let b = self.s.project_vector(&self.v).unwrap();
            self.s.engine().clock().advance(std::time::Duration::from_secs(1));
            if a == StepOutcome::Idle && b == StepOutcome::Idle {
                break;
            }
        }
    }

    fn head(&self) -> Option<u64> {
        self.s.engine().store().snapshot(&self.v).unwrap().head().unwrap()
    }

    fn hash(&self) -> String {
        dump_store(&self.s.engine().store().snapshot(&self.v).unwrap()).unwrap().sha256
    }

    fn backlink_pages(&self, id: &ObjectId) -> Vec<ObjectId> {
        let mut all: Vec<ObjectId> = self
            .s
            .page_backlinks(&self.reader, &self.v, id, None, 1000)
            .unwrap()
            .into_iter()
            .map(|b| b.from_page)
            .collect();
        all.sort();
        all
    }
}

#[test]
fn a_rename_keeps_every_link_and_backlink_on_a_generated_graph_of_10k_pages() {
    let w = world();
    const PAGES: usize = 10_000;
    // Every page links to the next two, and a tenth of them to the hub, so renaming costs the same at any size.
    let title = |i: usize| format!("Graph page {i}");
    let mut ids = vec![];
    let hub = w.create("Hub", "the hub");
    for chunk in (0..PAGES).collect::<Vec<_>>().chunks(500) {
        let ops: Vec<Op> = chunk
            .iter()
            .map(|i| {
                let hub_link = if i % 10 == 0 { " and [[Hub]]" } else { "" };
                Op::CreatePage {
                    id: ObjectId(format!("page{i:05}")),
                    title: title(*i),
                    body: format!("Links to [[{}]] and [[{}]]{hub_link}.", title((i + 1) % PAGES), title((i + 2) % PAGES)),
                    slug: None,
                    redirects: vec![],
                }
            })
            .collect();
        ids.extend(chunk.iter().map(|i| ObjectId(format!("page{i:05}"))));
        let out =
            w.s.propose(
                &w.human,
                &w.v,
                Proposal {
                    ops,
                    rationale: "graph".into(),
                    idempotency_key: format!("graph-{}", chunk[0]),
                },
                false,
            )
            .unwrap();
        assert_eq!(out.risk, RiskDecision::AutoCommit);
    }
    assert!(w.s.unresolved_page_links(&w.reader, &w.v, None, 10).unwrap().is_empty());
    let before: Vec<Vec<ObjectId>> = [0usize, 1, 5000, 9999].iter().map(|i| w.backlink_pages(&ids[*i])).collect();
    assert_eq!(before[0], vec![ids[9998].clone(), ids[9999].clone()]);
    let hub_before = w.backlink_pages(&hub);
    assert_eq!(hub_before.len(), 1000);

    // The rename touches only the page, its slug and its revision, whatever links to it.
    let at = w.head();
    w.rename(&hub, 1, "The Central Hub").unwrap();
    w.rename(&ids[5000], 1, "Renamed middle").unwrap();
    let snap = w.s.engine().store().snapshot(&w.v).unwrap();
    let events = snap.events(at).unwrap();
    assert_eq!(events.len(), 2, "each rename is one event");
    drop(snap);

    assert_eq!(w.backlink_pages(&hub), hub_before, "backlinks through the old slug still count");
    assert_eq!(w.backlink_pages(&ids[5000]), before[2]);
    assert_eq!(w.backlink_pages(&ids[0]), before[0]);
    // Outgoing links from a page naming the old title still reach the renamed page.
    let links = w.s.page_links(&w.reader, &w.v, &ids[4999], None, 10).unwrap();
    assert!(links.iter().all(|l| l.resolved), "{links:?}");
    assert!(links.iter().any(|l| l.to_page.as_ref() == Some(&ids[5000])));
    assert!(w.s.unresolved_page_links(&w.reader, &w.v, None, 10).unwrap().is_empty());
    // The old slug redirects, and the new one is current.
    let old = w.s.page_redirect(&w.reader, &w.v, "graph-page-5000").unwrap();
    assert!(old.redirected);
    assert_eq!((old.page_id, old.current_slug.as_str()), (ids[5000].clone(), "renamed-middle"));
    assert!(!w.s.page_redirect(&w.reader, &w.v, "renamed-middle").unwrap().redirected);
    // A rename of a renamed page keeps both old slugs.
    w.rename(&ids[5000], 2, "Renamed again").unwrap();
    assert_eq!(w.s.page_redirect(&w.reader, &w.v, "graph-page-5000").unwrap().current_slug, "renamed-again");
    assert_eq!(w.backlink_pages(&ids[5000]), before[2]);
    // Another page can't take an old slug, so a link can't change meaning under anyone.
    let taken = NewPage {
        title: "Graph page 5000".into(),
        slug: None,
        body: "x".into(),
        rationale: "r".into(),
        idempotency_key: "taken".into(),
    };
    assert!(matches!(w.s.propose_new_page(&w.human, &w.v, taken, false), Err(ServerError::Conflict(_))));
}

#[test]
fn a_stale_edit_is_a_conflict_with_the_current_page() {
    let w = world();
    let id = w.create("Page", "one");
    w.edit(&id, 1, "two").unwrap();
    match w.edit(&id, 1, "three") {
        Err(ServerError::StaleVersion { current, base, .. }) => {
            assert_eq!(base, 1);
            let Row::Page(p) = *current else { panic!("not a page") };
            assert_eq!((p.header.version, p.body.as_str()), (2, "two\n"));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(w.s.get_wiki_page(&w.reader, &w.v, &id, None).unwrap().content.body, "two\n");
}

#[test]
fn no_update_is_lost_under_100_concurrent_editors() {
    let w = world();
    let id = w.create("Contested", "start");
    let threads: Vec<_> = (0..100)
        .map(|i| {
            let (s, c, v, id) = (w.s.clone(), w.human.clone(), w.v.clone(), id.clone());
            std::thread::spawn(move || {
                let mut retries = 0;
                loop {
                    let page = s.get_wiki_page(&c, &v, &id, None).unwrap();
                    let body = format!("{}\nline {i}\n", page.content.body.trim_end());
                    let key = format!("editor-{i}-{}", page.current_revision);
                    match s.propose_page_edit(&c, &v, &id, page.current_revision, body, "r".into(), key, false) {
                        Ok(_) => return retries,
                        Err(ServerError::StaleVersion { .. }) => retries += 1,
                        Err(e) => panic!("{e}"),
                    }
                }
            })
        })
        .collect();
    let retries: u32 = threads.into_iter().map(|t| t.join().unwrap()).sum();
    let page = w.s.get_wiki_page(&w.reader, &w.v, &id, None).unwrap();
    assert_eq!(page.current_revision, 101, "one revision per editor, and {retries} retries");
    for i in 0..100 {
        assert!(page.content.body.contains(&format!("line {i}\n")), "editor {i}'s line is gone");
    }
    assert_eq!(w.s.list_page_revisions(&w.reader, &w.v, &id, None, 200).unwrap().len(), 101);
}

#[test]
fn a_plain_stale_race_lets_exactly_one_writer_through() {
    let w = world();
    let id = w.create("Raced", "start");
    let threads: Vec<_> = (0..100)
        .map(|i| {
            let (s, c, v, id) = (w.s.clone(), w.human.clone(), w.v.clone(), id.clone());
            std::thread::spawn(move || s.propose_page_edit(&c, &v, &id, 1, format!("from {i}"), "r".into(), format!("race-{i}"), false))
        })
        .collect();
    let results: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(results.iter().filter(|r| matches!(r, Err(ServerError::StaleVersion { .. }))).count(), 99);
}

#[test]
fn any_revision_can_be_read_and_a_revert_restores_it_exactly() {
    let w = world();
    let bodies = [
        "# Start\n\nFirst [[Other]].\n\n- a\n- b\n",
        "# Start\n\nFirst edited [[Third]].\n\n- a\n- b\n\nnew paragraph\n",
        "only this\n",
    ];
    let id = w.create("History", bodies[0]);
    w.edit(&id, 1, bodies[1]).unwrap();
    w.rename(&id, 2, "History renamed").unwrap();
    w.edit(&id, 3, bodies[2]).unwrap();
    let wanted = [
        (1, bodies[0], "History", "history"),
        (2, bodies[1], "History", "history"),
        (3, bodies[1], "History renamed", "history-renamed"),
        (4, bodies[2], "History renamed", "history-renamed"),
    ];
    for (revision, body, title, slug) in wanted {
        let v = w.s.get_wiki_page(&w.reader, &w.v, &id, Some(revision)).unwrap();
        assert_eq!(
            (v.content.revision, v.content.body.as_str(), v.content.title.as_str(), v.content.slug.as_str()),
            (revision, body, title, slug)
        );
        assert_eq!(v.current_revision, 4);
    }
    assert!(matches!(w.s.get_wiki_page(&w.reader, &w.v, &id, Some(9)), Err(ServerError::NotFound)));

    let first = w.s.get_wiki_page(&w.reader, &w.v, &id, Some(1)).unwrap().content;
    w.s.propose_page_revert(&w.human, &w.v, &id, 4, 1, "back".into(), "rv".into()).unwrap();
    let now = w.s.get_wiki_page(&w.reader, &w.v, &id, None).unwrap();
    assert_eq!(now.current_revision, 5);
    assert_eq!(now.content.restored_from, None, "a current read comes from the row");
    assert_eq!(
        (now.content.body.as_str(), now.content.title.as_str(), now.content.slug.as_str()),
        (bodies[0], "History", "history")
    );
    assert_eq!(now.content.blocks, first.blocks, "the same block ids, kinds and text");
    assert_eq!(w.s.get_wiki_page(&w.reader, &w.v, &id, Some(5)).unwrap().content.blocks, first.blocks);
    let links = w.s.page_links(&w.reader, &w.v, &id, None, 10).unwrap();
    assert_eq!(links.iter().map(|l| l.target.as_str()).collect::<Vec<_>>(), ["other"], "links are as they were");
    assert_eq!(now.redirects, ["history-renamed"], "slugs it has had keep working");
    let list = w.s.list_page_revisions(&w.reader, &w.v, &id, None, 10).unwrap();
    assert_eq!(list.last().unwrap().restored_from, Some(1));
    assert!(matches!(
        w.s.propose_page_revert(&w.human, &w.v, &id, 5, 99, "r".into(), "rv-missing".into())
            .unwrap()
            .risk,
        RiskDecision::Reject
    ));
}

#[test]
fn unchanged_blocks_keep_their_ids_and_a_diff_says_what_changed() {
    let w = world();
    let id = w.create("Diffed", "keep\n\nchange\n\ngo\n");
    let one = w.s.get_wiki_page(&w.reader, &w.v, &id, None).unwrap().content.blocks;
    let at = w.head();
    w.edit(&id, 1, "keep\n\nchanged\n\nnew one\n").unwrap();
    let two = w.s.get_wiki_page(&w.reader, &w.v, &id, None).unwrap().content.blocks;
    assert_eq!(one[0].id, two[0].id);
    assert_eq!(one[1].id, two[1].id, "an edit in place keeps the id");
    let snap = w.s.engine().store().snapshot(&w.v).unwrap();
    assert_eq!(snap.events(at).unwrap().len(), 1);
    drop(snap);
    let d = w.s.page_diff(&w.reader, &w.v, &id, 1, 2).unwrap();
    assert_eq!(d.title, None);
    assert!(d.blocks.iter().all(|c| c.block_id != one[0].id), "an unchanged block isn't in it");
    assert!(
        d.blocks
            .iter()
            .any(|c| c.change == "changed" && c.before.as_deref() == Some("change") && c.after.as_deref() == Some("changed"))
    );
    assert!(matches!(w.s.page_diff(&w.reader, &w.v, &id, 1, 7), Err(ServerError::NotFound)));
}

fn claim(id: &str, value: serde_json::Value, segment: &str, quote: &str) -> Op {
    Op::AddAssertion {
        id: ObjectId(id.into()),
        subject: "pump".into(),
        predicate: "weighs".into(),
        value,
        epistemic: Epistemic::Asserted,
        machine: false,
        confidence_permille: 1000,
        valid_from_ms: None,
        valid_to_ms: None,
        evidence: vec![(
            ObjectId(format!("{id}-e")),
            EvidenceInput {
                segment_id: ObjectId(segment.into()),
                quote: quote.into(),
                stance: Stance::Supports,
            },
        )],
    }
}

fn propose(w: &World, key: &str, ops: Vec<Op>) -> ProposalOutcome {
    w.s.propose(
        &w.human,
        &w.v,
        Proposal {
            ops,
            rationale: "r".into(),
            idempotency_key: key.into(),
        },
        false,
    )
    .unwrap()
}

#[test]
fn a_claim_reference_shows_the_current_claim_and_its_citations_and_marks_a_superseded_one() {
    let w = world();
    w.s.record_segment(
        &w.admin,
        &w.v,
        &ObjectId("seg".into()),
        &ObjectId("src".into()),
        None,
        "The pump weighs 12 kg, or 13 kg by now.",
    )
    .unwrap();
    propose(&w, "c1", vec![claim("old", json!(12), "seg", "weighs 12 kg")]);
    let id = w.create("Facts", "The pump: {{claim:old}}. Unknown: {{claim:nope}}.");
    let view = w.s.get_wiki_page(&w.reader, &w.v, &id, None).unwrap();
    assert_eq!(view.claims.len(), 2);
    assert_eq!(view.claims[0].status, ClaimRefStatus::Current);
    let c = view.claims[0].current.as_ref().unwrap();
    assert_eq!(c.assertion.value, json!(12));
    assert_eq!(c.evidence[0].evidence.quote, "weighs 12 kg");
    assert_eq!(c.evidence[0].segment.as_ref().unwrap().header.id.0, "seg");
    assert_eq!(view.claims[1].status, ClaimRefStatus::Missing);
    assert!(view.claims[1].current.is_none());

    propose(
        &w,
        "c2",
        vec![Op::SupersedeAssertion {
            target: ObjectId("old".into()),
            base_version: 1,
            replacement: Box::new(claim("new", json!(13), "seg", "13 kg")),
        }],
    );
    // Nothing was written to the page, and it shows the new fact anyway.
    let view = w.s.get_wiki_page(&w.reader, &w.v, &id, None).unwrap();
    assert_eq!(view.current_revision, 1);
    assert_eq!(view.claims[0].status, ClaimRefStatus::Superseded);
    assert_eq!(view.claims[0].claim_id.0, "old", "it still says which claim the page names");
    let c = view.claims[0].current.as_ref().unwrap();
    assert_eq!((c.assertion.header.id.0.as_str(), &c.assertion.value), ("new", &json!(13)));
    assert_eq!(c.evidence[0].evidence.quote, "13 kg");
    // A chain leads to its end.
    propose(
        &w,
        "c3",
        vec![Op::SupersedeAssertion {
            target: ObjectId("new".into()),
            base_version: 1,
            replacement: Box::new(claim("newest", json!(14), "seg", "weighs")),
        }],
    );
    let view = w.s.get_wiki_page(&w.reader, &w.v, &id, None).unwrap();
    assert_eq!(view.claims[0].current.as_ref().unwrap().assertion.header.id.0, "newest");
}

#[test]
fn a_new_page_is_searchable_by_its_title_and_its_blocks() {
    let w = world();
    let id = w.create(
        "Zanzibar Handbook",
        "# Ferries\n\nThe morning crossing leaves from Stonetown.\n\nSee [[Harbour Rules|the rules]].\n",
    );
    w.settle();
    let mut q = SearchRequest::new("zanzibar");
    q.mode = SearchMode::Lexical;
    let p = w.s.search(&w.reader, &w.v, &q).unwrap();
    assert_eq!(p.items[0].kind, ItemKind::Page);
    assert_eq!(p.items[0].id, id);
    assert_eq!(p.items[0].page_id.as_ref(), Some(&id));
    let mut q = SearchRequest::new("stonetown");
    q.mode = SearchMode::Lexical;
    let p = w.s.search(&w.reader, &w.v, &q).unwrap();
    assert_eq!(p.items[0].kind, ItemKind::Block);
    assert_eq!(p.items[0].page_id.as_ref(), Some(&id));
    assert_eq!(p.items[0].heading.as_deref(), Some("Zanzibar Handbook"));
    // A heading stands in its page's document, and link syntax isn't indexed.
    let mut q = SearchRequest::new("ferries");
    q.mode = SearchMode::Lexical;
    q.filter = LexicalFilter {
        kinds: vec![DocKind::Page],
        ..Default::default()
    };
    assert_eq!(w.s.search(&w.reader, &w.v, &q).unwrap().items[0].id, id);
    let mut q = SearchRequest::new("harbour");
    q.mode = SearchMode::Lexical;
    assert!(w.s.search(&w.reader, &w.v, &q).unwrap().items.is_empty(), "only the label shows");
    // A later edit's old words go, and a rename changes the page's document.
    w.edit(&id, 1, "Nothing about boats now.\n").unwrap();
    w.rename(&id, 2, "Pemba Handbook").unwrap();
    w.settle();
    let mut q = SearchRequest::new("stonetown");
    q.mode = SearchMode::Lexical;
    assert!(w.s.search(&w.reader, &w.v, &q).unwrap().items.is_empty());
    let mut q = SearchRequest::new("pemba");
    q.mode = SearchMode::Lexical;
    assert_eq!(w.s.search(&w.reader, &w.v, &q).unwrap().items[0].id, id);
    // An exact id is answered from the store.
    let p = w.s.search(&w.reader, &w.v, &SearchRequest::new(id.0.clone())).unwrap();
    assert_eq!(p.items[0].id, id);
}

#[test]
fn a_past_page_is_found_as_of_the_event_it_held_those_words() {
    let w = world();
    let id = w.create("Past Page", "the old wombat words");
    let at = w.head().unwrap();
    w.edit(&id, 1, "new words only").unwrap();
    let mut q = SearchRequest::new("wombat");
    q.mode = SearchMode::Lexical;
    q.as_of = Some(memex_types::AsOf::Seq(at));
    let p = w.s.search(&w.reader, &w.v, &q).unwrap();
    assert_eq!(p.items[0].kind, ItemKind::Block);
    assert_eq!(p.items[0].page_id.as_ref(), Some(&id));
    q.as_of = None;
    w.settle();
    assert!(w.s.search(&w.reader, &w.v, &q).unwrap().items.is_empty());
}

#[test]
fn a_page_reaches_the_vector_index_through_its_embed_jobs() {
    let w = world();
    let id = w.create("Vector Handbook", "# Part\n\nThe paragraph that gets embedded.\n");
    let block = w.s.get_wiki_page(&w.reader, &w.v, &id, None).unwrap().content.blocks[1].id.clone();
    w.settle();
    let mut q = SearchRequest::new("The paragraph that gets embedded.");
    q.mode = SearchMode::Vector;
    assert!(w.s.search(&w.reader, &w.v, &q).unwrap().items.is_empty(), "nothing is embedded yet");
    let mut jobs = 0;
    while let Some(job) = w.s.claim_job(&w.v, memex_core::EMBED_JOB, "t").unwrap() {
        assert!(w.s.run_embed_job(&w.v, &job).unwrap() > 0, "the job finds the page's text");
        assert!(w.s.complete_job(&w.v, &job, vec![]).unwrap());
        jobs += 1;
    }
    assert!(jobs > 0);
    w.settle();
    let items = w.s.search(&w.reader, &w.v, &q).unwrap().items;
    assert!(items.iter().any(|i| i.id == block && i.kind == ItemKind::Block), "{items:?}");
    assert!(items.iter().any(|i| i.id == id && i.kind == ItemKind::Page), "{items:?}");
}

#[test]
fn a_deleted_page_leaves_search_at_once_and_its_links_read_as_broken_until_a_page_takes_the_slug() {
    let w = world();
    let target = w.create("Target", "findable umbrella");
    let source = w.create("Source", "goes to [[Target]]");
    w.settle();
    let hash_before = w.hash();
    let out = w.s.propose_page_delete(&w.human, &w.v, &target, 1, "gone".into(), "del".into()).unwrap();
    assert_eq!(out.risk, RiskDecision::Review, "a delete always waits, even for a person");
    let cs = out.changeset.unwrap();
    assert_eq!(cs.state, ChangeSetState::Pending);
    assert!(w.s.get_wiki_page(&w.reader, &w.v, &target, None).is_ok());
    w.s.approve(&w.admin, &w.v, &cs.header.id, None).unwrap();
    // The index hasn't heard yet, and the fence drops it anyway.
    let mut q = SearchRequest::new("umbrella");
    q.mode = SearchMode::Lexical;
    assert!(w.s.search(&w.reader, &w.v, &q).unwrap().items.is_empty());
    assert!(matches!(w.s.get_wiki_page(&w.reader, &w.v, &target, None), Err(ServerError::NotFound)));
    let broken = w.s.unresolved_page_links(&w.reader, &w.v, None, 10).unwrap();
    assert_eq!(
        broken.iter().map(|b| (b.target.as_str(), &b.from_page)).collect::<Vec<_>>(),
        [("target", &source)]
    );
    assert_eq!(
        w.s.list_page_revisions(&w.reader, &w.v, &target, None, 10).unwrap().len(),
        1,
        "history outlives the page"
    );
    // The ChangeSet's revert restores all of it.
    w.s.revert(&w.admin, &w.v, &cs.header.id, "undo".into()).unwrap();
    assert!(w.s.unresolved_page_links(&w.reader, &w.v, None, 10).unwrap().is_empty());
    assert_eq!(w.backlink_pages(&target), vec![source.clone()]);
    assert_ne!(hash_before, w.hash(), "the ChangeSets themselves are history");
}

#[test]
fn a_link_written_first_resolves_when_its_page_appears() {
    let w = world();
    let source = w.create("Early", "See [[Later Page]] and [[Never Page]].");
    let broken = w.s.unresolved_page_links(&w.reader, &w.v, None, 10).unwrap();
    assert_eq!(broken.len(), 2);
    let later = w.create("Later Page", "here now");
    let broken = w.s.unresolved_page_links(&w.reader, &w.v, None, 10).unwrap();
    assert_eq!(broken.iter().map(|b| b.target.as_str()).collect::<Vec<_>>(), ["never-page"]);
    assert_eq!(w.backlink_pages(&later), vec![source.clone()]);
    let links = w.s.page_links(&w.reader, &w.v, &source, None, 10).unwrap();
    assert_eq!(links.iter().filter(|l| l.resolved).count(), 1);
    assert_eq!(links.iter().find(|l| l.resolved).unwrap().to_page.as_ref(), Some(&later));
}

#[test]
fn people_commit_and_agents_wait_through_the_risk_function() {
    let w = world();
    let new = |key: &str| NewPage {
        title: format!("Page {key}"),
        slug: None,
        body: "b".into(),
        rationale: "r".into(),
        idempotency_key: key.into(),
    };
    let (_, person) = w.s.propose_new_page(&w.human, &w.v, new("p"), false).unwrap();
    assert_eq!(person.risk, RiskDecision::AutoCommit);
    let (agent_page, agent) = w.s.propose_new_page(&w.agent, &w.v, new("a"), false).unwrap();
    assert_eq!(agent.risk, RiskDecision::Review);
    assert!(matches!(w.s.get_wiki_page(&w.reader, &w.v, &agent_page, None), Err(ServerError::NotFound)));
    w.s.approve(&w.admin, &w.v, &agent.changeset.unwrap().header.id, None).unwrap();
    assert_eq!(w.s.get_wiki_page(&w.reader, &w.v, &agent_page, None).unwrap().current_revision, 1);
    // An agent's edit, rename and revert all wait, and an edit approved late still can't lose an update.
    let edit =
        w.s.propose_page_edit(&w.agent, &w.v, &agent_page, 1, "agent".into(), "r".into(), "ae".into(), false)
            .unwrap();
    assert_eq!(edit.risk, RiskDecision::Review);
    let rename =
        w.s.propose_page_rename(&w.agent, &w.v, &agent_page, 1, "New".into(), None, "r".into(), "ar".into())
            .unwrap();
    assert_eq!(rename.risk, RiskDecision::Review);
    w.edit(&agent_page, 1, "person got there first").unwrap();
    assert!(matches!(
        w.s.approve(&w.admin, &w.v, &edit.changeset.unwrap().header.id, None),
        Err(ServerError::StaleVersion { .. })
    ));
    // Earned auto-commit counts for an agent's edit, but a delete always waits.
    w.s.record_audit_samples(&w.v, &w.agent.principal.id, "", "page", 100, 0, "test", "earned for this test")
        .unwrap();
    assert_eq!(
        w.s.propose_page_edit(&w.agent, &w.v, &agent_page, 2, "earned".into(), "r".into(), "ae2".into(), false)
            .unwrap()
            .risk,
        RiskDecision::AutoCommit
    );
    assert_eq!(
        w.s.propose_page_delete(&w.agent, &w.v, &agent_page, 3, "r".into(), "ad".into()).unwrap().risk,
        RiskDecision::Review
    );
    assert!(matches!(w.s.propose_new_page(&w.reader, &w.v, new("x"), false), Err(ServerError::Forbidden)));
}

#[test]
fn bad_cases_are_listed_refused_or_harmless() {
    let w = world();
    // Broken links and references are listed, and nothing else minds.
    let id = w.create("Broken", "[[Nowhere]] and {{claim:gone}} and [[?!]] and [[open and {{claim:bad id}}");
    let broken = w.s.unresolved_page_links(&w.reader, &w.v, None, 10).unwrap();
    let mut targets: Vec<&str> = broken.iter().map(|b| b.target.as_str()).collect();
    targets.sort_unstable();
    assert_eq!(targets, ["gone", "nowhere"]);
    assert_eq!(w.s.get_wiki_page(&w.reader, &w.v, &id, None).unwrap().current_revision, 1);
    assert_eq!(w.s.page_backlinks(&w.reader, &w.v, &id, None, 10).unwrap().len(), 0);

    // Oversized: refused before anything is written, so it never reaches the log.
    let head = w.head();
    let big = "x".repeat(memex_core::page::MAX_PAGE_BYTES + 1);
    let new = NewPage {
        title: "Huge".into(),
        slug: None,
        body: big.clone(),
        rationale: "r".into(),
        idempotency_key: "huge".into(),
    };
    assert!(matches!(w.s.propose_new_page(&w.human, &w.v, new, false), Err(ServerError::Invalid(_))));
    assert!(matches!(w.edit(&id, 1, &big), Err(ServerError::Invalid(_))));
    assert_eq!(w.head(), head);
    // Too many blocks or links for a page within the byte limit, and a bad title or slug, are rejected.
    let many_blocks = "x\n\n".repeat(memex_core::page::MAX_BLOCKS + 1);
    assert_eq!(w.edit(&id, 1, &many_blocks).unwrap().risk, RiskDecision::Reject);
    let many_links: String = (0..=memex_core::page::MAX_LINKS).map(|i| format!("[[p{i}]] ")).collect();
    assert_eq!(w.edit(&id, 1, &many_links).unwrap().risk, RiskDecision::Reject);
    let titled = |title: &str, slug: Option<&str>, key: &str| NewPage {
        title: title.into(),
        slug: slug.map(String::from),
        body: "b".into(),
        rationale: "r".into(),
        idempotency_key: key.into(),
    };
    assert_eq!(
        w.s.propose_new_page(&w.human, &w.v, titled("?!", None, "t1"), false).unwrap().1.risk,
        RiskDecision::Reject
    );
    assert_eq!(
        w.s.propose_new_page(&w.human, &w.v, titled("  ", None, "t2"), false).unwrap().1.risk,
        RiskDecision::Reject
    );
    assert_eq!(
        w.s.propose_new_page(&w.human, &w.v, titled("Ok", Some("Not A Slug"), "t3"), false)
            .unwrap()
            .1
            .risk,
        RiskDecision::Reject
    );
    assert!(matches!(
        w.s.get_wiki_page(&w.reader, &w.v, &ObjectId("nope".into()), None),
        Err(ServerError::NotFound)
    ));
    assert!(matches!(w.s.page_redirect(&w.reader, &w.v, "nope"), Err(ServerError::NotFound)));
    // One page can't change twice in one ChangeSet.
    let out = propose(
        &w,
        "twice",
        vec![
            Op::EditPage {
                id: id.clone(),
                base_version: 1,
                body: "a".into(),
            },
            Op::EditPage {
                id: id.clone(),
                base_version: 1,
                body: "b".into(),
            },
        ],
    );
    assert_eq!(out.risk, RiskDecision::Reject);
}

fn fused(id: &str, kind: DocKind, score: f32) -> Fused {
    Fused {
        id: ObjectId(id.into()),
        kind,
        score,
        lexical_rank: Some(1),
        lexical_score: Some(score),
        vector_rank: None,
        vector_score: None,
    }
}

#[test]
fn a_neighborhood_shows_links_both_ways_through_old_slugs_and_names_broken_ones() {
    let w = world();
    let target = w.create("Target", "the target");
    let citing = w.create("Citing", "cites [[Target]] and [[Nowhere]]");
    let kinds = [WIKI_LINK, "contradicts"];
    let at = |id: &ObjectId| w.s.neighborhood(&w.reader, &w.v, id, &kinds, None, 50).unwrap();
    let out = at(&citing);
    assert_eq!(out.len(), 2);
    assert!(out.iter().all(|n| n.direction == "out" && n.from == citing));
    let found = out.iter().find(|n| n.neighbor.is_some()).unwrap();
    assert_eq!(found.neighbor.as_ref(), Some(&target));
    assert_eq!(found.title.as_deref(), Some("Target"));
    let broken = out.iter().find(|n| n.neighbor.is_none()).unwrap();
    assert_eq!(broken.to, slug_target("nowhere"));
    let back = at(&target);
    assert_eq!(back.len(), 1);
    assert_eq!((back[0].direction, &back[0].from), ("in", &citing));
    assert_eq!(back[0].title.as_deref(), Some("Citing"));
    // A link written to an old slug still arrives after a rename.
    w.rename(&target, 1, "Moved").unwrap();
    assert_eq!(at(&target).len(), 1, "through the old slug");
    assert_eq!(at(&citing).iter().find(|n| n.neighbor.is_some()).unwrap().title.as_deref(), Some("Moved"));
    // Pages come back in id order with a cursor, and a kind not asked for stays out.
    let one = w.s.neighborhood(&w.reader, &w.v, &citing, &kinds, None, 1).unwrap();
    let rest = w.s.neighborhood(&w.reader, &w.v, &citing, &kinds, Some(&one[0].edge_id), 1).unwrap();
    assert_ne!(one[0].edge_id, rest[0].edge_id);
    assert!(w.s.neighborhood(&w.reader, &w.v, &citing, &["claim_ref"], None, 50).unwrap().is_empty());
    // Nothing there, and nothing for someone who can't see the vault.
    assert!(matches!(
        w.s.neighborhood(&w.reader, &w.v, &ObjectId("nope".into()), &kinds, None, 50),
        Err(ServerError::NotFound)
    ));
    let stranger = Caller {
        grants: vec![],
        ..w.reader.clone()
    };
    assert!(matches!(
        w.s.neighborhood(&stranger, &w.v, &citing, &kinds, None, 50),
        Err(ServerError::NotFound)
    ));
}

#[test]
fn a_neighborhood_reads_with_few_store_calls() {
    let w = world();
    let hub = w.create("Hub", "just the hub");
    for i in 0..30 {
        w.create(&format!("Spoke {i}"), "links to [[Hub]]");
    }
    let reads = || w.s.engine().store().counts().reads.load(std::sync::atomic::Ordering::Relaxed);
    let before = reads();
    let out = w.s.neighborhood(&w.reader, &w.v, &hub, &[WIKI_LINK], None, 100).unwrap();
    assert_eq!(out.len(), 30);
    assert!(reads() - before <= 5, "{} reads", reads() - before);
}

#[test]
fn link_cycles_terminate_and_expansion_finds_neighbours_both_ways() {
    let w = world();
    let a = w.create("Alpha", "alpha words link [[Beta]]");
    let b = w.create("Beta", "beta words link [[Gamma]] and back to [[Alpha]]");
    let c = w.create("Gamma", "gamma words link [[Alpha]] and itself [[Gamma]]");
    let block_of = |id: &ObjectId| w.s.get_wiki_page(&w.reader, &w.v, id, None).unwrap().content.blocks[0].id.clone();
    let snap = w.s.engine().store().snapshot(&w.v).unwrap();
    for hops in [1, 2, 3, 50] {
        let seed = vec![fused(&block_of(&a).0, DocKind::Block, 1.0)];
        let (out, added) = expand(&snap, seed, hops).unwrap();
        assert_eq!(added, 2, "hops {hops}: each other page once, however long the cycle goes on");
        let ids: Vec<&ObjectId> = out.iter().map(|f| &f.id).collect();
        assert!(ids.contains(&&block_of(&b)) && ids.contains(&&block_of(&c)), "{ids:?}");
        assert_eq!(out[0].id, block_of(&a), "the seed stays first");
    }
    // A segment has no edges, so it adds nothing.
    let (out, added) = expand(&snap, vec![fused("seg", DocKind::Segment, 1.0)], 1).unwrap();
    assert_eq!((out.len(), added), (1, 0));
}

#[test]
fn expansion_is_bounded_and_reaches_through_old_slugs_and_claims() {
    let w = world();
    let mut body = String::from("hub words");
    for i in 0..30 {
        w.create(&format!("Spoke {i}"), &format!("spoke {i} words"));
        body.push_str(&format!(" [[Spoke {i}]]"));
    }
    let hub = w.create("Hub", &body);
    let renamed = w.create("Before", "the page that moved");
    let linker = w.create("Linker", "through [[Before]]");
    w.rename(&renamed, 1, "After").unwrap();
    let hub_block = w.s.get_wiki_page(&w.reader, &w.v, &hub, None).unwrap().content.blocks[0].id.clone();
    let linker_block = w.s.get_wiki_page(&w.reader, &w.v, &linker, None).unwrap().content.blocks[0].id.clone();
    let renamed_block = w.s.get_wiki_page(&w.reader, &w.v, &renamed, None).unwrap().content.blocks[0].id.clone();
    let snap = w.s.engine().store().snapshot(&w.v).unwrap();
    let (out, added) = expand(&snap, vec![fused(&hub_block.0, DocKind::Block, 1.0)], 1).unwrap();
    assert_eq!(added, NEIGHBOURS_PER_SEED, "a page that links to 30 adds at most this many");
    assert!(added <= MAX_ADDED);
    assert!((out[1].score - 0.5).abs() < f32::EPSILON, "a neighbour scores half its seed");
    let (out, _) = expand(&snap, vec![fused(&linker_block.0, DocKind::Block, 1.0)], 1).unwrap();
    assert!(out.iter().any(|f| f.id == renamed_block), "a link by the old slug still leads there");
    let (out, _) = expand(&snap, vec![fused(&renamed_block.0, DocKind::Block, 1.0)], 1).unwrap();
    assert!(
        out.iter().any(|f| f.id == linker_block),
        "a backlink through the old slug counts, and gives the block holding it"
    );
    drop(snap);
    // Edges read for a search don't grow with the hub's size, and the fence checks what's added.
    let mut q = SearchRequest::new("hub");
    q.mode = SearchMode::Lexical;
    q.expand = Expand::On;
    q.debug = true;
    w.settle();
    let p = w.s.search(&w.reader, &w.v, &q).unwrap();
    assert!(p.debug.unwrap().expanded > 0);
    assert!(p.items.iter().any(|i| i.text.contains("spoke")));
    let mut q2 = q.clone();
    q2.expand = Expand::Off;
    assert_eq!(w.s.search(&w.reader, &w.v, &q2).unwrap().debug.unwrap().expanded, 0);
    let mut q3 = q.clone();
    q3.expand = Expand::Auto;
    q3.text = "hub".into();
    assert_eq!(
        w.s.search(&w.reader, &w.v, &q3).unwrap().debug.unwrap().expanded,
        0,
        "the rule doesn't class this as relational"
    );
}

#[test]
fn expansion_follows_a_claim_to_the_pages_that_cite_it_and_back() {
    let w = world();
    w.s.record_segment(&w.admin, &w.v, &ObjectId("seg".into()), &ObjectId("src".into()), None, "The pump weighs 12 kg.")
        .unwrap();
    propose(&w, "c", vec![claim("cited", json!(12), "seg", "weighs 12 kg")]);
    let page = w.create("Citing", "Weight: {{claim:cited}} and some words.");
    let block = w.s.get_wiki_page(&w.reader, &w.v, &page, None).unwrap().content.blocks[0].id.clone();
    let snap = w.s.engine().store().snapshot(&w.v).unwrap();
    let (out, _) = expand(&snap, vec![fused("cited", DocKind::Claim, 1.0)], 1).unwrap();
    assert!(out.iter().any(|f| f.id == block && f.kind == DocKind::Block), "claim to page: {out:?}");
    let (out, _) = expand(&snap, vec![fused(&block.0, DocKind::Block, 1.0)], 1).unwrap();
    assert!(out.iter().any(|f| f.id.0 == "cited" && f.kind == DocKind::Claim), "page to claim: {out:?}");
}

#[test]
fn export_then_import_reproduces_content_and_links_in_another_vault() {
    let w = world();
    let a = w.create(
        "Ålborg Notes",
        "# Top\n\nSee [[Second Page|the second]] and {{claim:abc}}.\n\n```\ncode [[not a link]]\n```\n",
    );
    let b = w.create("Second Page", "Back to [[Ålborg Notes]] and on to [[Missing]], old name [[Third Old]].");
    let c = w.create("Third Old", "third");
    w.rename(&c, 1, "Third New").unwrap();
    let other = w.s.create_vault(&w.admin, "other").unwrap().id;
    let links_of = |vault: &VaultId, id: &ObjectId| -> Vec<(String, String, bool)> {
        let mut l: Vec<_> =
            w.s.page_links(&w.admin, vault, id, None, 50)
                .unwrap()
                .into_iter()
                .map(|x| (x.kind, x.target, x.resolved))
                .collect();
        l.sort();
        l
    };
    let view = |vault: &VaultId, id: &ObjectId| {
        let v = w.s.get_wiki_page(&w.admin, vault, id, None).unwrap();
        (
            v.content.title,
            v.current_slug,
            v.redirects,
            v.content.body,
            v.content.blocks.iter().map(|b| (b.kind, b.text.clone())).collect::<Vec<_>>(),
        )
    };
    // In the order that leaves links waiting, which is the hard one.
    let mut made = vec![];
    for (i, id) in [&a, &b, &c].into_iter().enumerate() {
        let file = w.s.export_page(&w.reader, &w.v, id).unwrap();
        let out = w.s.import_page(&w.admin, &other, &file, None, "import".into(), format!("import-{i}")).unwrap();
        assert!(out.created);
        assert_eq!(out.outcome.risk, RiskDecision::AutoCommit);
        made.push((id.clone(), out.page_id));
    }
    for (source, copy) in &made {
        assert_eq!(view(&w.v, source), view(&other, copy));
        assert_eq!(links_of(&w.v, source), links_of(&other, copy));
        assert_eq!(
            w.s.page_backlinks(&w.admin, &w.v, source, None, 50)
                .unwrap()
                .iter()
                .map(|b| b.via_slug.clone())
                .collect::<Vec<_>>(),
            w.s.page_backlinks(&w.admin, &other, copy, None, 50)
                .unwrap()
                .iter()
                .map(|b| b.via_slug.clone())
                .collect::<Vec<_>>()
        );
    }
    // Exporting the copy gives the same file.
    assert_eq!(
        w.s.export_page(&w.admin, &w.v, &a).unwrap(),
        w.s.export_page(&w.admin, &other, &made[0].1).unwrap()
    );
    // Importing onto a page that exists needs its version.
    let file = w.s.export_page(&w.reader, &w.v, &a).unwrap();
    assert!(matches!(
        w.s.import_page(&w.admin, &other, &file, None, "r".into(), "again".into()),
        Err(ServerError::Conflict(_))
    ));
    let edited = file.replace("See", "Now see");
    let out = w.s.import_page(&w.admin, &other, &edited, Some(1), "r".into(), "again2".into()).unwrap();
    assert!(!out.created);
    assert!(w.s.get_wiki_page(&w.admin, &other, &made[0].1, None).unwrap().content.body.contains("Now see"));
    let retitled = edited.replace("Ålborg Notes", "Other Title");
    assert!(matches!(
        w.s.import_page(&w.admin, &other, &retitled, Some(2), "r".into(), "retitled".into()),
        Err(ServerError::Invalid(_))
    ));
    assert!(matches!(
        w.s.import_page(&w.admin, &other, "no front matter or heading", None, "r".into(), "bad".into()),
        Err(ServerError::Invalid(_))
    ));
}

#[test]
fn a_page_from_before_spec_022_stays_readable_and_joins_in_on_its_first_edit() {
    let w = world();
    let lease = w.s.vault_lease_for_tests(&w.v).unwrap();
    w.s.engine()
        .apply(
            &lease,
            memex_types::EventDraft {
                actor: "t".into(),
                kind: "page.created".into(),
                schema_version: 1,
                payload: json!({"id": "old", "title": "Old Page", "body": "ancient quokka [[Elsewhere]]"}),
            },
        )
        .unwrap();
    let id = ObjectId("old".into());
    let view = w.s.get_wiki_page(&w.reader, &w.v, &id, None).unwrap();
    assert_eq!(
        (view.current_slug.as_str(), view.content.body.as_str(), view.content.blocks.len()),
        ("", "ancient quokka [[Elsewhere]]", 0)
    );
    w.settle();
    let mut q = SearchRequest::new("quokka");
    q.mode = SearchMode::Lexical;
    assert_eq!(w.s.search(&w.reader, &w.v, &q).unwrap().items[0].id, id);
    w.edit(&id, 1, "modern quokka [[Elsewhere]]").unwrap();
    let view = w.s.get_wiki_page(&w.reader, &w.v, &id, None).unwrap();
    assert_eq!(
        (view.current_slug.as_str(), view.content.blocks.len(), view.current_revision),
        ("old-page", 1, 2)
    );
    assert_eq!(w.s.page_links(&w.reader, &w.v, &id, None, 5).unwrap()[0].target, "elsewhere");
    assert!(matches!(w.s.page_redirect(&w.reader, &w.v, "old-page"), Ok(r) if !r.redirected));
    // The row's own header says the rest of the story.
    let snap = w.s.engine().store().snapshot(&w.v).unwrap();
    let Some(Row::Page(p)) = snap
        .get(&RowKey {
            table: Table::Page,
            id: id.clone(),
        })
        .unwrap()
    else {
        panic!()
    };
    assert_eq!(p.header.status, Status::Active);
    assert_eq!(snap.edges_touching(&[slug_target("elsewhere")], WIKI_LINK).unwrap().len(), 1);
}
