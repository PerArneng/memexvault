//! Durable jobs and upload through the facade, in memory.
use memex_core::testing::{ManualClock, MemBlobStore, MemRegistry, MemStore, SeededRandom};
use memex_core::{BlobStore, Caller, NewToken, Server, ServerEdges, ServerError, Snapshot, Store, UploadLimits, UploadRequest, jobs};
use memex_types::{Grant, Header, JobState, Mutation, ObjectId, PageRow, PrincipalKind, Row, RowKey, Scope, Status, Table, VaultId};
use serde_json::json;
use std::io::Read;
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
}

struct World {
    s: Server<Fakes>,
    admin: Caller,
    writer: Caller,
    reader: Caller,
    v: VaultId,
}

fn world() -> World {
    let s = Server::<Fakes>::new(
        MemRegistry::new(),
        MemStore::new(),
        MemBlobStore::new(ManualClock::starting_at(0)),
        ManualClock::starting_at(1_000_000),
        SeededRandom::new(9),
        memex_core::testing::mem_search(),
    );
    let admin = s.authenticate(&s.bootstrap("root").unwrap().token).unwrap();
    let v = s.create_vault(&admin, "v").unwrap().id;
    let make = |name: &str, scope| {
        let p = s.create_principal(&admin, name, PrincipalKind::Human, false).unwrap();
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
    let (writer, reader) = (make("w", Scope::Propose), make("r", Scope::Read));
    World { s, admin, writer, reader, v }
}

fn request(name: &str) -> UploadRequest {
    UploadRequest {
        name: name.into(),
        trust_tier: "unverified".into(),
        metadata: json!({"k": "v"}),
        replaces: None,
    }
}

const LIMITS: UploadLimits = UploadLimits {
    max_bytes: 1000,
    max_open_jobs: 3,
};
const PDF: &[u8] = b"%PDF-1.7 pretend content";

fn upload(w: &World, name: &str, bytes: &[u8]) -> Result<memex_core::UploadOutcome, ServerError> {
    w.s.upload(&w.writer, &w.v, request(name), LIMITS, &mut &bytes[..])
}

fn derived(job: &str, id: &str) -> Mutation {
    let header = Header {
        id: ObjectId(id.into()),
        creator: job.into(),
        status: Status::Active,
        version: 1,
        acl_id: None,
    };
    Mutation::Put(Row::Page(PageRow {
        header,
        title: job.into(),
        body: "derived".into(),
        updated_at_ms: 0,
    }))
}

fn advance(w: &World, ms: u64) {
    w.s.engine().clock().advance(Duration::from_millis(ms));
}

#[test]
fn an_upload_creates_a_source_and_its_job_in_one_step() {
    let w = world();
    let head_before = w.s.engine().store().snapshot(&w.v).unwrap().head().unwrap();
    let out = upload(&w, "report.pdf", PDF).unwrap();
    assert!(!out.existing);
    assert_eq!(
        (out.source.media_type.as_str(), out.source.size, out.source.version_no),
        ("application/pdf", PDF.len() as u64, 1)
    );
    assert_eq!(out.source.logical_id, out.source.header.id);
    assert_eq!(out.job.as_ref().unwrap().kind, "rendition");
    assert_eq!(
        w.s.engine().store().snapshot(&w.v).unwrap().head().unwrap(),
        Some(head_before.map_or(0, |h| h + 1))
    );
    assert_eq!(w.s.get_source(&w.reader, &w.v, &out.source.header.id).unwrap(), out.source);
}

#[test]
fn the_same_bytes_twice_make_one_blob_one_source_and_one_job() {
    let w = world();
    let a = upload(&w, "a.pdf", PDF).unwrap();
    let b = upload(&w, "other-name.pdf", PDF).unwrap();
    assert!(b.existing);
    assert_eq!(a.source.header.id, b.source.header.id);
    assert_eq!(a.job.unwrap().header.id, b.job.unwrap().header.id);
    assert_eq!(w.s.list_sources(&w.reader, &w.v, None, 10).unwrap().len(), 1);
    assert_eq!(w.s.list_jobs(&w.reader, &w.v, None, None, None, 10).unwrap().len(), 1);
}

#[test]
fn a_new_version_shares_the_document_and_counts_up() {
    let w = world();
    let first = upload(&w, "a.pdf", PDF).unwrap().source;
    let mut req = request("a.pdf");
    req.replaces = Some(first.header.id.clone());
    let second = w.s.upload(&w.writer, &w.v, req, LIMITS, &mut &b"%PDF-1.7 second"[..]).unwrap().source;
    assert_eq!((second.logical_id.clone(), second.version_no), (first.header.id.clone(), 2));
    let versions = w.s.source_versions(&w.reader, &w.v, &second.header.id).unwrap();
    assert_eq!(versions.iter().map(|s| s.version_no).collect::<Vec<_>>(), vec![1, 2]);
    let mut bad = request("x.pdf");
    bad.replaces = Some(ObjectId("nope".into()));
    assert!(matches!(
        w.s.upload(&w.writer, &w.v, bad, LIMITS, &mut &b"%PDF-1.7 third"[..]),
        Err(ServerError::NotFound)
    ));
}

#[test]
fn bad_uploads_create_nothing_and_leave_no_blob() {
    let w = world();
    let blobs = |w: &World| w.s.blobs().list(&w.v, None, 10).unwrap().len();
    assert!(matches!(upload(&w, "a.pdf", b""), Err(ServerError::Invalid(_))));
    assert!(matches!(
        upload(&w, "a.png", &[0x89, b'P', b'N', b'G', 0, 1, 2]),
        Err(ServerError::UnsupportedMedia(_))
    ));
    assert_eq!(blobs(&w), 0);
    let big = vec![b'x'; 1001];
    assert!(matches!(upload(&w, "a.txt", &big), Err(ServerError::TooLarge { limit: 1000 })));
    assert!(matches!(upload(&w, "  ", PDF), Err(ServerError::Invalid(_))));
    let mut req = request("a.pdf");
    req.trust_tier = "made-up".into();
    assert!(matches!(
        w.s.upload(&w.writer, &w.v, req, LIMITS, &mut PDF.to_vec().as_slice()),
        Err(ServerError::Invalid(_))
    ));
    assert_eq!(blobs(&w), 0);
    assert!(w.s.list_sources(&w.reader, &w.v, None, 10).unwrap().is_empty());
    assert!(w.s.list_jobs(&w.reader, &w.v, None, None, None, 10).unwrap().is_empty());
}

#[test]
fn a_client_that_disconnects_mid_upload_creates_no_source() {
    struct Disconnects(u8);
    impl Read for Disconnects {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.0 == 0 {
                return Err(std::io::Error::other("connection reset"));
            }
            self.0 -= 1;
            buf[..5].copy_from_slice(b"%PDF-");
            Ok(5)
        }
    }
    let w = world();
    assert!(matches!(
        w.s.upload(&w.writer, &w.v, request("a.pdf"), LIMITS, &mut Disconnects(2)),
        Err(ServerError::Invalid(_))
    ));
    assert!(w.s.list_sources(&w.reader, &w.v, None, 10).unwrap().is_empty());
    assert_eq!(w.s.blobs().list(&w.v, None, 10).unwrap().len(), 0);
}

#[test]
fn wrong_scope_is_refused_before_any_bytes_are_read() {
    let w = world();
    struct Panics;
    impl Read for Panics {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            panic!("must not be read");
        }
    }
    assert!(matches!(
        w.s.upload(&w.reader, &w.v, request("a.pdf"), LIMITS, &mut Panics),
        Err(ServerError::Forbidden)
    ));
    let other = w.s.create_vault(&w.admin, "other").unwrap().id;
    assert!(matches!(
        w.s.upload(&w.writer, &other, request("a.pdf"), LIMITS, &mut Panics),
        Err(ServerError::NotFound)
    ));
}

#[test]
fn a_saturated_queue_refuses_new_uploads_without_reading_them() {
    let w = world();
    for i in 0..3u8 {
        upload(&w, "a.pdf", &[b"%PDF-1.7 ".as_slice(), &[i + 48]].concat()).unwrap();
    }
    struct Panics;
    impl Read for Panics {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            panic!("must not be read");
        }
    }
    assert!(matches!(
        w.s.upload(&w.writer, &w.v, request("b.pdf"), LIMITS, &mut Panics),
        Err(ServerError::Busy)
    ));
    // Finishing work makes room.
    let job = w.s.claim_job(&w.v, "rendition", "w1").unwrap().unwrap();
    assert!(w.s.complete_job(&w.v, &job, vec![]).unwrap());
    upload(&w, "b.pdf", b"%PDF-1.7 fourth").unwrap();
}

#[test]
fn a_job_runs_once_and_commits_its_result_with_its_completion() {
    let w = world();
    let up = upload(&w, "a.pdf", PDF).unwrap();
    let job = w.s.claim_job(&w.v, "rendition", "w1").unwrap().unwrap();
    assert_eq!(job.header.id, up.job.unwrap().header.id);
    assert!(w.s.claim_job(&w.v, "rendition", "w2").unwrap().is_none(), "a live lease blocks a second worker");
    assert!(w.s.claim_job(&w.v, "other-kind", "w2").unwrap().is_none());
    assert!(w.s.complete_job(&w.v, &job, vec![derived("j", "result-1")]).unwrap());
    let snap = w.s.engine().store().snapshot(&w.v).unwrap();
    assert!(
        snap.get(&RowKey {
            table: Table::Page,
            id: ObjectId("result-1".into())
        })
        .unwrap()
        .is_some()
    );
    let Some(Row::Job(done)) = snap
        .get(&RowKey {
            table: Table::Job,
            id: job.header.id.clone(),
        })
        .unwrap()
    else {
        panic!("job")
    };
    assert_eq!(done.state, JobState::Done);
    drop(snap);
    assert!(w.s.claim_job(&w.v, "rendition", "w2").unwrap().is_none());
}

#[test]
fn an_expired_lease_is_recovered_and_the_old_worker_can_not_commit() {
    let w = world();
    upload(&w, "a.pdf", PDF).unwrap();
    let old = w.s.claim_job(&w.v, "rendition", "w1").unwrap().unwrap();
    advance(&w, jobs::LEASE_MS as u64 - 1);
    assert!(w.s.claim_job(&w.v, "rendition", "w2").unwrap().is_none());
    advance(&w, 1);
    let new = w.s.claim_job(&w.v, "rendition", "w2").unwrap().unwrap();
    assert_eq!(new.attempts.len(), 2);
    assert!(new.attempts[0].error.is_some());
    // The first worker wakes up: its result must not appear, so there's no duplicate.
    assert!(!w.s.complete_job(&w.v, &old, vec![derived("j", "from-old")]).unwrap());
    assert!(!w.s.fail_job(&w.v, &old, "late", true).unwrap());
    assert!(w.s.complete_job(&w.v, &new, vec![derived("j", "from-new")]).unwrap());
    let snap = w.s.engine().store().snapshot(&w.v).unwrap();
    assert!(
        snap.get(&RowKey {
            table: Table::Page,
            id: ObjectId("from-old".into())
        })
        .unwrap()
        .is_none()
    );
    assert!(
        snap.get(&RowKey {
            table: Table::Page,
            id: ObjectId("from-new".into())
        })
        .unwrap()
        .is_some()
    );
}

#[test]
fn a_same_owner_restart_does_not_let_a_stale_attempt_commit() {
    let w = world();
    upload(&w, "a.pdf", PDF).unwrap();
    let old = w.s.claim_job(&w.v, "rendition", "w1").unwrap().unwrap();
    advance(&w, jobs::LEASE_MS as u64);
    let new = w.s.claim_job(&w.v, "rendition", "w1").unwrap().unwrap();
    assert!(!w.s.complete_job(&w.v, &old, vec![]).unwrap());
    assert!(w.s.complete_job(&w.v, &new, vec![]).unwrap());
}

#[test]
fn an_always_failing_job_reaches_dead_letter_and_does_not_block_others() {
    let w = world();
    let bad = upload(&w, "bad.pdf", b"%PDF-1.7 bad").unwrap().job.unwrap();
    let good = upload(&w, "good.pdf", b"%PDF-1.7 good").unwrap().job.unwrap();
    let mut attempts = 0;
    let mut good_done = false;
    for _ in 0..200 {
        match w.s.claim_job(&w.v, "rendition", "w").unwrap() {
            Some(job) if job.header.id == bad.header.id => {
                attempts += 1;
                assert!(w.s.fail_job(&w.v, &job, "parser crashed", true).unwrap());
            }
            Some(job) => {
                assert_eq!(job.header.id, good.header.id);
                assert!(w.s.complete_job(&w.v, &job, vec![]).unwrap());
                good_done = true;
            }
            None => advance(&w, 3_700_000),
        }
        if good_done && w.s.get_job(&w.reader, &w.v, &bad.header.id).unwrap().state == JobState::Dead {
            break;
        }
    }
    assert!(good_done);
    assert_eq!(attempts, 5);
    let dead = w.s.list_jobs(&w.reader, &w.v, None, Some(JobState::Dead), None, 10).unwrap();
    assert_eq!(dead.len(), 1);
    assert_eq!(dead[0].attempts.len(), 5);
    assert!(dead[0].attempts.iter().all(|a| a.error.as_deref() == Some("parser crashed")));
    // A person can retry it.
    assert!(matches!(w.s.retry_job(&w.reader, &w.v, &bad.header.id), Err(ServerError::Forbidden)));
    assert_eq!(w.s.retry_job(&w.writer, &w.v, &bad.header.id).unwrap().state, JobState::Queued);
    assert_eq!(w.s.claim_job(&w.v, "rendition", "w").unwrap().unwrap().header.id, bad.header.id);
}

#[test]
fn cancel_and_retry_follow_the_rules() {
    let w = world();
    let job = upload(&w, "a.pdf", PDF).unwrap().job.unwrap();
    assert!(matches!(w.s.retry_job(&w.writer, &w.v, &job.header.id), Err(ServerError::Conflict(_))));
    assert_eq!(w.s.cancel_job(&w.writer, &w.v, &job.header.id).unwrap().state, JobState::Cancelled);
    assert!(w.s.claim_job(&w.v, "rendition", "w").unwrap().is_none());
    assert!(matches!(w.s.cancel_job(&w.writer, &w.v, &job.header.id), Err(ServerError::Conflict(_))));
    w.s.retry_job(&w.writer, &w.v, &job.header.id).unwrap();
    let running = w.s.claim_job(&w.v, "rendition", "w").unwrap().unwrap();
    // Cancelling running work: the worker's commit is then refused.
    w.s.cancel_job(&w.writer, &w.v, &job.header.id).unwrap();
    assert!(!w.s.complete_job(&w.v, &running, vec![derived("j", "late")]).unwrap());
}

#[test]
fn lists_filter_and_page_at_10000_rows_while_jobs_change_state() {
    let w = world();
    let big = UploadLimits {
        max_bytes: 1000,
        max_open_jobs: 1_000_000,
    };
    for i in 0..10_000 {
        w.s.upload(&w.writer, &w.v, request("d.pdf"), big, &mut format!("%PDF-1.7 {i}").as_bytes())
            .unwrap();
    }
    // Every other job finishes while a reader pages through them.
    let (mut seen, mut after) = (std::collections::HashSet::new(), None);
    let mut pages = 0;
    loop {
        let page = w.s.list_jobs(&w.reader, &w.v, Some("rendition"), None, after.as_ref(), 500).unwrap();
        let Some(last) = page.last() else { break };
        after = Some(last.header.id.clone());
        for j in &page {
            assert!(seen.insert(j.header.id.clone()), "a job appeared twice");
        }
        pages += 1;
        for _ in 0..50 {
            let job = w.s.claim_job(&w.v, "rendition", "w").unwrap().unwrap();
            w.s.complete_job(&w.v, &job, vec![]).unwrap();
        }
    }
    assert_eq!((seen.len(), pages), (10_000, 20));
    let done = w.s.list_jobs(&w.reader, &w.v, None, Some(JobState::Done), None, 2000).unwrap();
    assert_eq!(done.len(), 1000);
    let queued = w.s.list_jobs(&w.reader, &w.v, Some("rendition"), Some(JobState::Queued), None, 20_000).unwrap();
    assert_eq!(queued.len(), 9000);
    assert!(w.s.list_jobs(&w.reader, &w.v, Some("nope"), None, None, 10).unwrap().is_empty());
    let (mut count, mut after) = (0, None);
    loop {
        let page = w.s.list_sources(&w.reader, &w.v, after.as_ref(), 1000).unwrap();
        let Some(last) = page.last() else { break };
        after = Some(last.header.id.clone());
        count += page.len();
    }
    assert_eq!(count, 10_000);
}

#[test]
fn raising_trust_on_an_untrusted_source_goes_to_review_and_metadata_edits_do_not() {
    use memex_types::{Op, RiskDecision};
    let w = world();
    let mut req = request("a.pdf");
    req.trust_tier = "untrusted".into();
    let s = w.s.upload(&w.writer, &w.v, req, LIMITS, &mut PDF.to_vec().as_slice()).unwrap().source;
    let propose = |key: &str, op: Op| {
        w.s.propose(
            &w.writer,
            &w.v,
            memex_core::Proposal {
                ops: vec![op],
                rationale: "r".into(),
                idempotency_key: key.into(),
            },
            false,
        )
        .unwrap()
    };
    let promote = propose(
        "a",
        Op::SetSourceTrust {
            id: s.header.id.clone(),
            base_version: 1,
            trust_tier: "trusted".into(),
        },
    );
    assert_eq!(promote.risk, RiskDecision::Review);
    assert_eq!(w.s.get_source(&w.reader, &w.v, &s.header.id).unwrap().trust_tier, "untrusted");
    let meta = propose(
        "b",
        Op::SetSourceMetadata {
            id: s.header.id.clone(),
            base_version: 1,
            metadata: json!({"team": "x"}),
        },
    );
    assert_eq!(meta.risk, RiskDecision::AutoCommit);
    assert_eq!(w.s.get_source(&w.reader, &w.v, &s.header.id).unwrap().metadata, json!({"team": "x"}));
    w.s.approve(&w.admin, &w.v, &promote.changeset.unwrap().header.id, None).unwrap_err();
}
