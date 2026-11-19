//! Erasure through the facade, in memory (spec 030): what it removes, what it leaves flagged, that the log replays to the same state, and that the preview lists exactly what goes.
use memex_core::testing::{ManualClock, MemBlobStore, MemRegistry, MemStore, ScriptedAgent, SeededRandom};
use memex_core::{
    BlobStore, Caller, ERASE_CASCADE, EXTRACT_JOB, ExtractJobError, NewPage, NewToken, Proposal, RENDITION_JOB, RenditionInput, Server, ServerEdges,
    ServerError, Snapshot, StepOutcome, Store, UploadLimits, UploadRequest, all_events, dump_store, replay,
};
use memex_types::{
    AgentError, AgentReply, AgentRequest, AgentUsage, ChangeSetState, ERASE_JOB, Epistemic, EvidenceInput, Grant, ObjectId, Op, PrincipalKind, RiskDecision,
    Row, RowKey, Scope, SearchRequest, Stance, Table, VaultId,
};
use serde_json::{Value, json};
use std::io::Read;

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

const MARKER: &str = "ZQXMARKER7731";

struct World {
    s: Server<Fakes>,
    admin: Caller,
    agent: Caller,
    reader: Caller,
    v: VaultId,
}

fn filler() -> String {
    "Detta är en lång mening om ingenting särskilt som bara fyller ut stycket med ord. ".repeat(12)
}

fn world() -> World {
    world_seeded(30)
}

fn world_seeded(seed: u64) -> World {
    let s = Server::<Fakes>::new(
        MemRegistry::new(),
        MemStore::new(),
        MemBlobStore::new(ManualClock::starting_at(0)),
        ManualClock::starting_at(1_000_000),
        SeededRandom::new(seed),
        memex_core::testing::mem_search(),
    );
    let admin = s.authenticate(&s.bootstrap("root").unwrap().token).unwrap();
    let v = s.create_vault(&admin, "v").unwrap().id;
    s.set_extraction(Some(memex_core::ExtractionLimits::default()));
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
    let agent = make("bot", PrincipalKind::Agent, Scope::Admin);
    let reader = make("reader", PrincipalKind::Human, Scope::Propose);
    World { s, admin, agent, reader, v }
}

fn reply(doc: Value) -> Result<AgentReply, AgentError> {
    Ok(AgentReply {
        json: doc.to_string(),
        usage: AgentUsage {
            input_tokens: 10,
            output_tokens: 10,
            thinking_tokens: 0,
        },
        tool_calls: 0,
    })
}

fn first_sentence(text: &str) -> String {
    text.split_inclusive(". ").next().unwrap().trim().to_string()
}

/// A claim per segment, quoting its first sentence, and a Person named in the first segment.
fn claims_and_a_person(req: &AgentRequest) -> Result<AgentReply, AgentError> {
    let doc: Value = serde_json::from_str(req.input.split("\n\nYour previous").next().unwrap()).unwrap();
    let segments: Vec<(String, String)> = doc["segments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| (s["id"].as_str().unwrap().to_string(), s["text"].as_str().unwrap().to_string()))
        .collect();
    let claims: Vec<Value> = segments
        .iter()
        .map(|(id, text)| {
            let q = first_sentence(text);
            json!({"segment_id": id, "quote": q, "subject": q, "predicate": "säger", "value": q})
        })
        .collect();
    let (id, text) = &segments[0];
    let name = first_sentence(text).trim_end_matches('.').to_string();
    reply(json!({"claims": claims, "entities": {"Person": [{"segment_id": id, "quote": first_sentence(text), "fields": {"name": name}}]}}))
}

impl World {
    fn ingest(&self, name: &str, text: &str) -> ObjectId {
        let req = UploadRequest {
            name: name.into(),
            trust_tier: "unverified".into(),
            metadata: json!({}),
            replaces: None,
        };
        let source = self
            .s
            .upload(&self.admin, &self.v, req, UploadLimits::default(), &mut text.as_bytes())
            .unwrap()
            .source
            .header
            .id;
        let job = self.s.claim_job(&self.v, RENDITION_JOB, "parser").unwrap().unwrap();
        let rows = self
            .s
            .build_rendition(
                &self.v,
                &source,
                RenditionInput::Plain {
                    text: text.into(),
                    parser: "plain-1".into(),
                },
            )
            .unwrap();
        assert!(self.s.complete_job(&self.v, &job, rows).unwrap());
        let agent = ScriptedAgent::answering(claims_and_a_person);
        let job = self.s.claim_job(&self.v, EXTRACT_JOB, "w").unwrap().unwrap();
        let rows = self.s.run_extraction_job(&self.v, &job, &agent).unwrap();
        assert!(self.s.complete_job(&self.v, &job, rows).unwrap());
        source
    }

    fn table(&self, table: Table) -> Vec<Row> {
        let snap = self.s.engine().store().snapshot(&self.v).unwrap();
        let (mut out, mut after) = (vec![], None);
        loop {
            let page = snap.rows(table, after.as_ref()).unwrap();
            let Some(last) = page.last() else { return out };
            after = Some(last.header().id.clone());
            out.extend(page);
        }
    }

    fn segments_of(&self, source: &ObjectId) -> Vec<(ObjectId, String)> {
        self.table(Table::Segment)
            .into_iter()
            .filter_map(|r| match r {
                Row::Segment(s) if &s.source_id == source => Some((s.header.id, s.text)),
                _ => None,
            })
            .collect()
    }

    fn approve_all(&self) {
        for c in self.s.list_changesets(&self.admin, &self.v, Some(ChangeSetState::Pending), None, 1000).unwrap() {
            self.s.approve(&self.admin, &self.v, &c.header.id, None).unwrap();
        }
    }

    /// Runs the projectors and every embedding job until nothing is left to do.
    fn settle(&self) {
        for _ in 0..100 {
            let a = self.s.project_lexical(&self.v).unwrap();
            let b = self.s.project_vector(&self.v).unwrap();
            let mut embedded = false;
            while let Some(job) = self.s.claim_job(&self.v, memex_core::EMBED_JOB, "t").unwrap() {
                self.s.run_embed_job(&self.v, &job).unwrap();
                assert!(self.s.complete_job(&self.v, &job, vec![]).unwrap());
                embedded = true;
            }
            self.s.engine().clock().advance(std::time::Duration::from_secs(60));
            if a == StepOutcome::Idle && b == StepOutcome::Idle && !embedded {
                return;
            }
        }
        panic!("the projectors never settled");
    }

    /// Keyword hits, and hits of any signal whose text holds the words. A vector search always finds a nearest neighbour.
    fn hits(&self, text: &str) -> usize {
        let mut q = SearchRequest::new(text);
        q.mode = memex_types::SearchMode::Lexical;
        let keyword = self.s.search(&self.admin, &self.v, &q).unwrap().items.len();
        let any = self.s.search(&self.admin, &self.v, &SearchRequest::new(text)).unwrap().items;
        keyword + any.iter().filter(|i| i.text.contains(text)).count()
    }

    /// Everything the vault holds, as bytes: rows, the log, and every blob.
    fn all_bytes(&self) -> String {
        let snap = self.s.engine().store().snapshot(&self.v).unwrap();
        let mut out = dump_store(&snap).unwrap().text;
        for e in all_events(&snap).unwrap() {
            out.push_str(&serde_json::to_string(&e.payload).unwrap());
        }
        for b in self.s.blobs().list(&self.v, None, 10_000).unwrap() {
            let mut bytes = vec![];
            self.s.blobs().read(&self.v, &b.hash, 0, None).unwrap().read_to_end(&mut bytes).unwrap();
            out.push_str(&String::from_utf8_lossy(&bytes));
        }
        out
    }

    fn run_erase_job(&self) {
        self.settle();
        let job = self.s.claim_job(&self.v, ERASE_JOB, "w").unwrap().expect("an erase job");
        let rows = self.s.run_erase_job(&self.v, &job).unwrap();
        assert!(self.s.complete_job(&self.v, &job, rows).unwrap());
    }
}

/// Two documents, one with the marker. Claims and a Person from each, a claim citing both, and a page citing a claim from the marked one.
struct Fixture {
    w: World,
    marked: ObjectId,
    other: ObjectId,
    mixed: ObjectId,
    page: ObjectId,
}

fn fixture() -> Fixture {
    let w = world();
    let marked_text = format!(
        "Anna {MARKER} byggde fartyget i Stockholm. {f}\n\nFartyget seglade med {MARKER} till Riga. {f}",
        f = filler()
    );
    let other_text = format!("Erik Lind ritade hamnen i Göteborg. {f}\n\nHamnen fick en ny kaj år 1890. {f}", f = filler());
    let marked = w.ingest("marked.txt", &marked_text);
    let other = w.ingest("other.txt", &other_text);
    w.approve_all();
    // A person's claim standing on both documents.
    let (m_seg, m_text) = w.segments_of(&marked)[0].clone();
    let (o_seg, o_text) = w.segments_of(&other)[0].clone();
    let mixed = ObjectId("mixed-claim".into());
    let out =
        w.s.propose(
            &w.admin,
            &w.v,
            Proposal {
                ops: vec![Op::AddAssertion {
                    id: mixed.clone(),
                    subject: "hamnar".into(),
                    predicate: "jämförs".into(),
                    value: json!("två"),
                    epistemic: Epistemic::Asserted,
                    machine: false,
                    confidence_permille: 900,
                    valid_from_ms: None,
                    valid_to_ms: None,
                    evidence: vec![
                        (
                            ObjectId("ev-m".into()),
                            EvidenceInput {
                                segment_id: m_seg,
                                quote: first_sentence(&m_text),
                                stance: Stance::Supports,
                            },
                        ),
                        (
                            ObjectId("ev-o".into()),
                            EvidenceInput {
                                segment_id: o_seg,
                                quote: first_sentence(&o_text),
                                stance: Stance::Supports,
                            },
                        ),
                    ],
                }],
                rationale: "both".into(),
                idempotency_key: "mixed".into(),
            },
            false,
        )
        .unwrap();
    assert_eq!(out.risk, RiskDecision::AutoCommit, "{:?}", out.validators);
    let cited = w
        .table(Table::Assertion)
        .into_iter()
        .find_map(|r| match r {
            Row::Assertion(a) if a.subject.contains(MARKER) => Some(a.header.id),
            _ => None,
        })
        .unwrap();
    let (page, out) =
        w.s.propose_new_page(
            &w.admin,
            &w.v,
            NewPage {
                title: "Notes".into(),
                slug: None,
                body: format!("A claim: {{{{claim:{cited}}}}} and nothing more."),
                rationale: "r".into(),
                idempotency_key: "page".into(),
            },
            false,
        )
        .unwrap();
    assert_eq!(out.risk, RiskDecision::AutoCommit);
    w.settle();
    Fixture { w, marked, other, mixed, page }
}

#[test]
fn erasure_removes_the_marker_everywhere_and_the_log_replays_to_the_same_state() {
    let Fixture { w, marked, other, mixed, page } = fixture();
    assert!(w.hits(MARKER) > 0, "the marker is searchable before");
    assert!(w.all_bytes().contains(MARKER));
    let entities_before = w.table(Table::Entity).len();
    assert_eq!(entities_before, 2);

    let preview = w.s.erasure_preview(&w.admin, &w.v, &marked).unwrap();
    // An agent's proposal waits for a person, and an agent can't approve it, whatever its scope.
    let proposed = w.s.propose_erasure(&w.agent, &w.v, &marked, "asked to forget it".into()).unwrap();
    let cs = proposed.changeset.unwrap();
    assert_eq!((proposed.risk, cs.state), (RiskDecision::Review, ChangeSetState::Pending));
    assert!(matches!(w.s.approve(&w.agent, &w.v, &cs.header.id, None), Err(ServerError::Forbidden)));
    assert!(w.s.claim_job(&w.v, ERASE_JOB, "w").unwrap().is_none(), "nothing runs before a person approves");
    let approved = w.s.approve(&w.admin, &w.v, &cs.header.id, None).unwrap();
    assert_eq!(approved.state, ChangeSetState::Committed);

    // What the log records is exactly what the preview listed.
    let erasures = w.s.erasures(&w.admin, &w.v, None, 10).unwrap();
    assert_eq!(erasures.len(), 1);
    let record = &erasures[0].record;
    assert_eq!(record.changeset_id, cs.header.id);
    assert_eq!((&record.removed, &record.rewritten), (&preview.removed, &preview.rewritten));
    assert_eq!(
        (&record.flagged_claims, &record.flagged_pages),
        (&preview.flagged_claims, &preview.flagged_pages)
    );
    assert_eq!(record.flagged_claims, vec![mixed.clone()]);
    assert_eq!(record.flagged_pages, vec![page.clone()]);
    let snap = w.s.engine().store().snapshot(&w.v).unwrap();
    for k in &record.removed {
        assert!(snap.get(k).unwrap().is_none(), "{k:?} is gone");
    }
    for k in &record.rewritten {
        assert!(snap.get(k).unwrap().is_some(), "{k:?} stays");
    }
    drop(snap);
    assert!(
        record.removed.iter().any(|k| k.table == Table::Entity),
        "the Person named only in the marked document goes"
    );
    assert_eq!(w.table(Table::Entity).len(), 1, "the other document's Person stays");

    w.run_erase_job();
    assert_eq!(w.hits(MARKER), 0);
    assert!(w.hits("Göteborg") > 0, "the other document is untouched");
    assert!(!w.all_bytes().contains(MARKER), "no row, event or blob holds the marker");
    assert!(matches!(w.s.get_source(&w.admin, &w.v, &marked), Err(ServerError::NotFound)));
    assert!(w.s.get_source(&w.admin, &w.v, &other).is_ok());
    // The claim on both documents stands on what's left.
    let snap = w.s.engine().store().snapshot(&w.v).unwrap();
    assert_eq!(snap.evidence_of(std::slice::from_ref(&mixed)).unwrap().len(), 1);
    assert!(matches!(
        snap.get(&RowKey {
            table: Table::Assertion,
            id: mixed.clone()
        })
        .unwrap(),
        Some(Row::Assertion(_))
    ));

    // The rewritten log replays into an empty store to exactly this state.
    let events = all_events(&snap).unwrap();
    let live = dump_store(&snap).unwrap();
    drop(snap);
    let fresh = MemStore::new();
    let lease = fresh.open_vault(&w.v).unwrap();
    replay(events.clone(), &fresh, &lease).unwrap();
    let replayed = dump_store(&fresh.snapshot(&w.v).unwrap()).unwrap();
    assert_eq!(replayed.sha256, live.sha256, "replay gives the same state");
    assert!(events.iter().any(|e| e.kind == memex_types::SOURCE_ERASED));

    // It can't be undone, and erasing again finds nothing.
    assert!(matches!(
        w.s.revert(&w.admin, &w.v, &cs.header.id, "undo".into()),
        Err(ServerError::NotReversible(_))
    ));
    assert!(matches!(w.s.erasure_preview(&w.admin, &w.v, &marked), Err(ServerError::NotFound)));
    assert_eq!(ERASE_CASCADE.len(), 6);
}

#[test]
fn the_erase_job_waits_for_the_indexes_to_apply_the_erasure() {
    let Fixture { w, marked, .. } = fixture();
    let cs = w.s.propose_erasure(&w.admin, &w.v, &marked, "gone".into()).unwrap().changeset.unwrap();
    assert_eq!(cs.state, ChangeSetState::Pending, "a person's own erasure waits for approval too");
    w.s.approve(&w.admin, &w.v, &cs.header.id, None).unwrap();
    let job = w.s.claim_job(&w.v, ERASE_JOB, "w").unwrap().unwrap();
    assert!(matches!(w.s.run_erase_job(&w.v, &job), Err(ExtractJobError::Wait(_))));
}

#[test]
fn an_erasure_shares_its_changeset_with_nothing() {
    let Fixture { w, marked, other, .. } = fixture();
    let out =
        w.s.propose(
            &w.admin,
            &w.v,
            Proposal {
                ops: vec![Op::EraseSource { id: marked }, Op::EraseSource { id: other }],
                rationale: "both".into(),
                idempotency_key: "two".into(),
            },
            false,
        )
        .unwrap();
    assert_eq!(out.risk, RiskDecision::Reject);
    let missing =
        w.s.propose(
            &w.admin,
            &w.v,
            Proposal {
                ops: vec![Op::EraseSource { id: ObjectId("nope".into()) }],
                rationale: "r".into(),
                idempotency_key: "nope".into(),
            },
            false,
        )
        .unwrap();
    assert_eq!(missing.risk, RiskDecision::Reject);
}

#[test]
fn reconcile_finds_nothing_on_a_settled_vault_and_names_what_breaks() {
    let Fixture { w, marked, .. } = fixture();
    let report = w.s.reconcile(&w.admin, &w.v, None, true, false).unwrap();
    assert!(report.clean(), "{report:?}");
    assert_eq!(report.log.tables_sha256, report.log.replayed_sha256);
    assert_eq!(report.indexes.len(), 4);
    assert!(report.indexes.iter().all(|i| i.matches), "{:?}", report.indexes);
    assert_eq!(report.blobs.missing.len() + report.blobs.corrupt.len(), 0);
    assert!(report.blobs.referenced > 0 && report.blobs.stored >= report.blobs.referenced);

    // An earlier state can be named, for comparing with a restored backup.
    let at = report.log.head.unwrap() - 3;
    let earlier = w.s.reconcile(&w.admin, &w.v, Some(at), false, false).unwrap();
    assert_eq!(earlier.log.at.as_ref().map(|(s, _)| *s), Some(at));
    assert_ne!(earlier.log.at.as_ref().unwrap().1, report.log.tables_sha256);

    // A blob that's gone is named, and only a backup can bring it back.
    let sha = w.s.get_source(&w.admin, &w.v, &marked).unwrap().sha256;
    w.s.blobs().delete(&w.v, &memex_types::BlobHash::parse(&sha).unwrap()).unwrap();
    let broken = w.s.reconcile(&w.admin, &w.v, None, false, true).unwrap();
    assert_eq!(broken.blobs.missing, vec![sha]);
    assert!(!broken.clean());
    assert!(broken.repaired.is_empty(), "the indexes were fine, so nothing was rebuilt");

    // Reconciling is an admin's.
    assert!(matches!(w.s.reconcile(&w.reader, &w.v, None, false, false), Err(ServerError::Forbidden)));
}

#[test]
fn the_sweep_removes_old_orphans_only() {
    let w = world();
    // The blob store's clock starts at 0 and the server's at 1,000 s, so every blob starts 1,000 s old.
    let grace = 1_500_000;
    let young = w.s.blobs().put(&w.v, &mut "an orphan".as_bytes()).unwrap();
    let report = w.s.sweep(&w.admin, &w.v, grace).unwrap();
    assert_eq!((report.removed, report.kept_young), (0, 1));
    w.s.engine().clock().advance(std::time::Duration::from_secs(600));
    let source = w.ingest("kept.txt", &format!("Ett dokument som stannar. {}", filler()));
    let report = w.s.sweep(&w.admin, &w.v, grace).unwrap();
    assert_eq!(report.removed, 1, "{report:?}");
    assert!(w.s.blobs().verify(&w.v, &young.hash).is_err());
    let sha = w.s.get_source(&w.admin, &w.v, &source).unwrap().sha256;
    assert!(
        w.s.blobs().verify(&w.v, &memex_types::BlobHash::parse(&sha).unwrap()).is_ok(),
        "a referenced blob stays however old"
    );
}

#[test]
fn status_and_metrics_show_lag_queues_and_sizes() {
    let Fixture { w, .. } = fixture();
    let status = w.s.status(&w.admin).unwrap();
    let v = &status.vaults[0];
    assert_eq!((v.keyword.lag, v.vector.lag), (0, 0));
    assert!(v.blobs >= 2 && v.blob_bytes > 0);
    let text = w.s.metrics(&w.admin).unwrap();
    assert!(text.contains(&format!("memex_keyword_lag_events{{vault=\"{}\"}} 0", w.v.0)), "{text}");
    assert!(text.contains("memex_open_jobs{"));
    assert!(matches!(w.s.status(&w.agent), Err(ServerError::Forbidden)), "it's for server admins");
}

/// Vault names are unique, so each restore here names its own.
fn restore(
    s: &Server<Fakes>,
    bytes: &[u8],
    target: memex_core::RestoreTarget,
) -> Result<(memex_types::VaultRow, memex_core::backup::BackupSummary), ServerError> {
    let owned = bytes.to_vec();
    let name = format!("restored {}", s.vault_ids().unwrap().len());
    s.restore_vault(
        "operator",
        &mut || Ok(Box::new(std::io::Cursor::new(owned.clone())) as Box<dyn Read>),
        target,
        Some(&name),
    )
}

#[test]
fn a_backup_restores_elsewhere_to_the_same_state_and_a_bad_one_is_refused() {
    let Fixture { w, .. } = fixture();
    let mut bytes = vec![];
    let (header, summary) = w.s.backup_vault(&w.v, &mut bytes).unwrap();
    let live = dump_store(&w.s.engine().store().snapshot(&w.v).unwrap()).unwrap();
    assert_eq!(header.dump_sha256, live.sha256);
    assert_eq!(summary.events, header.head.unwrap() + 1);
    assert!(summary.blobs >= 3 && summary.embeddings > 0, "{summary:?}");
    assert_eq!(summary.bytes, bytes.len() as u64);

    // Another server, with nothing in it, under the vault's own id.
    let other = world_seeded(31);
    let (row, restored) = restore(&other.s, &bytes, memex_core::RestoreTarget::Original).unwrap();
    assert_eq!(row.id, w.v);
    assert_eq!(restored.sha256, live.sha256);
    let report = other.s.reconcile(&other.admin, &row.id, None, true, false).unwrap();
    assert!(report.clean(), "{report:?}");
    assert_eq!(report.log.tables_sha256, live.sha256);
    assert_eq!(report.blobs.stored, report.blobs.referenced);
    // Its indexes build from the tables, and find what the original did.
    other.s.rebuild_lexical(&row.id);
    for _ in 0..100 {
        if other.s.project_lexical(&row.id).unwrap() == StepOutcome::Idle {
            break;
        }
        other.s.engine().clock().advance(std::time::Duration::from_secs(60));
    }
    let mut q = SearchRequest::new(MARKER);
    q.mode = memex_types::SearchMode::Lexical;
    let found = |s: &Server<Fakes>, c: &Caller, v: &VaultId| -> Vec<ObjectId> { s.search(c, v, &q).unwrap().items.into_iter().map(|i| i.id).collect() };
    assert_eq!(found(&other.s, &other.admin, &row.id), found(&w.s, &w.admin, &w.v));

    // An existing vault is never overwritten unless asked, and a new id always works.
    assert!(matches!(
        restore(&other.s, &bytes, memex_core::RestoreTarget::Original),
        Err(ServerError::Conflict(_))
    ));
    let (copy, _) = restore(&other.s, &bytes, memex_core::RestoreTarget::NewId).unwrap();
    assert_ne!(copy.id, w.v);
    let (again, _) = restore(&other.s, &bytes, memex_core::RestoreTarget::Replace).unwrap();
    assert_eq!(again.id, w.v);

    // One flipped byte, a cut, or a file that isn't a backup: refused, and nothing is created.
    let before = other.s.vault_ids().unwrap().len();
    let mut flipped = bytes.clone();
    let middle = flipped.len() / 2;
    flipped[middle] ^= 0x20;
    for bad in [flipped, bytes[..bytes.len() - 10].to_vec(), b"not a backup\n".to_vec()] {
        let err = restore(&other.s, &bad, memex_core::RestoreTarget::NewId).unwrap_err();
        assert!(matches!(err, ServerError::Invalid(ref m) if m.contains("damaged")), "{err:?}");
    }
    assert_eq!(other.s.vault_ids().unwrap().len(), before);
}

#[test]
fn an_entity_merged_into_another_takes_its_claims_with_it_and_the_log_still_replays() {
    let Fixture { w, marked, .. } = fixture();
    let people: Vec<memex_types::EntityRow> = w
        .table(Table::Entity)
        .into_iter()
        .filter_map(|r| match r {
            Row::Entity(e) => Some(e),
            _ => None,
        })
        .collect();
    let (gone, kept): (Vec<_>, Vec<_>) = people.into_iter().partition(|e| serde_json::to_string(e).unwrap().contains(MARKER));
    let (source, target) = (&gone[0], &kept[0]);
    let merge =
        w.s.propose(
            &w.admin,
            &w.v,
            Proposal {
                ops: vec![Op::MergeEntities {
                    source: source.header.id.clone(),
                    source_version: source.header.version,
                    target: target.header.id.clone(),
                    target_version: target.header.version,
                }],
                rationale: "the same person".into(),
                idempotency_key: "merge".into(),
            },
            false,
        )
        .unwrap()
        .changeset
        .unwrap();
    w.s.approve(&w.admin, &w.v, &merge.header.id, None).unwrap();
    w.settle();
    let target_row = |w: &World| match w
        .s
        .engine()
        .store()
        .snapshot(&w.v)
        .unwrap()
        .get(&RowKey {
            table: Table::Entity,
            id: target.header.id.clone(),
        })
        .unwrap()
    {
        Some(Row::Entity(e)) => e,
        other => panic!("{other:?}"),
    };
    assert!(
        serde_json::to_string(&target_row(&w)).unwrap().contains(MARKER),
        "the merge brought the claim's value in"
    );
    let cs = w.s.propose_erasure(&w.admin, &w.v, &marked, "gone".into()).unwrap().changeset.unwrap();
    w.s.approve(&w.admin, &w.v, &cs.header.id, None).unwrap();
    w.run_erase_job();
    let after = target_row(&w);
    assert!(!serde_json::to_string(&after).unwrap().contains(MARKER), "{after:?}");
    assert!(!w.all_bytes().contains(MARKER));
    let snap = w.s.engine().store().snapshot(&w.v).unwrap();
    let fresh = MemStore::new();
    let lease = fresh.open_vault(&w.v).unwrap();
    replay(all_events(&snap).unwrap(), &fresh, &lease).unwrap();
    assert_eq!(dump_store(&fresh.snapshot(&w.v).unwrap()).unwrap().sha256, dump_store(&snap).unwrap().sha256);
}
