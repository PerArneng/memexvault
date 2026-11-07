//! The activity stream over real sockets and real HTTP, in memory: fake edges, with the server's own routing, buffers and hub. Tests on Postgres are in `memexvault/tests/activity.rs`.
// These tests run a server and read a socket with time limits.
#![allow(clippy::disallowed_methods)]
#[path = "marker_fixture.rs"]
mod marker_fixture;
#[path = "sse_client.rs"]
mod sse_client;

use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use marker_fixture::{MARKER, fill_with_marker, plain, reply};
use memex_api::frame_of;
use memex_core::testing::{FakeRasterizer, ManualClock, MemBlobStore, MemRegistry, MemStore, ScriptedAgent, SeededRandom};
use memex_core::{
    ActivityLimits, BufferLimits, Caller, EMBED_JOB, EXTRACT_JOB, ExtractionLimits, NewEntity, NewPage, NewToken, Proposal, RENDITION_JOB, REVIEW_JOB,
    RenditionInput, Server, ServerEdges, Snapshot, Store, TRANSCRIBE_JOB, TriggerAudit, UploadLimits, UploadRequest,
};
use memex_types::{
    AgentError, AgentReply, AgentUsage, Epistemic, EvidenceInput, Grant, MediaFacts, ObjectId, Op, PrincipalKind, Scope, Stance, Table, TimedDocument,
    TimedWord, TranscribeUsage, VaultId,
};
use serde_json::{Value, json};
use sse_client::{Frame, Sse, open, seq_of};
use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tower::ServiceExt;

const WAIT: Duration = Duration::from_secs(20);

struct Fakes;

impl ServerEdges for Fakes {
    type Registry = MemRegistry;
    type Store = MemStore;
    type Blobs = MemBlobStore<ManualClock>;
    type Clock = ManualClock;
    type Seed = SeededRandom;
    type Rasterizer = FakeRasterizer;
    type Lexical = memex_core::testing::MemLexical;
    type Vector = memex_core::testing::MemVector;
    type Embedder = memex_core::testing::FakeEmbedder;
    type Reranker = memex_core::testing::FakeReranker;
    type Caches = memex_core::NoCaches;
}

fn fast() -> ActivityLimits {
    ActivityLimits {
        poll_ms: 10,
        recheck_ms: 20,
        heartbeat_ms: 60_000,
        ..ActivityLimits::default()
    }
}

struct World {
    s: Arc<Server<Fakes>>,
    root: String,
    admin: Caller,
    v: VaultId,
    addr: SocketAddr,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    server_done: Option<std::thread::JoinHandle<()>>,
}

fn new_server() -> Arc<Server<Fakes>> {
    Arc::new(Server::new(
        MemRegistry::new(),
        MemStore::new(),
        MemBlobStore::new(ManualClock::starting_at(0)),
        ManualClock::starting_at(1_700_000_000_000),
        SeededRandom::new(38),
        memex_core::testing::mem_search(),
    ))
}

fn world(limits: ActivityLimits) -> World {
    let s = new_server();
    s.set_activity_limits(limits);
    let root = s.bootstrap("root").unwrap().token;
    let admin = s.authenticate(&root).unwrap();
    let v = s.create_vault(&admin, "v").unwrap().id;
    let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = probe.local_addr().unwrap();
    drop(probe);
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let server = s.clone();
    let server_done = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        rt.block_on(async {
            let shutdown = async {
                let _ = stopped.await;
            };
            memex_http::serve_with(server, axum::Router::new(), &addr.to_string(), shutdown).await.unwrap();
        });
    });
    for _ in 0..400 {
        if std::net::TcpStream::connect(addr).is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    World {
        s,
        root,
        admin,
        v,
        addr,
        stop: Some(stop),
        server_done: Some(server_done),
    }
}

impl Drop for World {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        // A never-reading client in a test holds its connection until its stall limit, so this doesn't wait.
        drop(self.server_done.take());
    }
}

impl World {
    fn path(&self) -> String {
        format!("/api/v1/vaults/{}/activity", self.v)
    }

    fn follow(&self, token: &str) -> Sse {
        self.follow_with(token, &[])
    }

    fn follow_with(&self, token: &str, more: &[(&str, &str)]) -> Sse {
        let auth = format!("Bearer {token}");
        let mut headers = vec![("Authorization", auth.as_str())];
        headers.extend_from_slice(more);
        open(self.addr, &self.path(), &headers)
    }

    fn token(&self, name: &str, grants: Vec<Grant>) -> (String, String, String) {
        let p = self.s.create_principal(&self.admin, name, PrincipalKind::Human, false).unwrap();
        let t = self
            .s
            .create_token(
                &self.admin,
                NewToken {
                    principal_id: Some(p.id.clone()),
                    name: "t".into(),
                    grants,
                    expires_at_ms: None,
                },
            )
            .unwrap();
        (t.token, t.row.id, p.id)
    }

    fn reader(&self, name: &str) -> (String, String, String) {
        self.token(
            name,
            vec![Grant {
                vault: self.v.clone(),
                scope: Scope::Read,
            }],
        )
    }

    fn upload_to(&self, vault: &VaultId, text: &str) -> ObjectId {
        let req = UploadRequest {
            name: "d.txt".into(),
            trust_tier: "unverified".into(),
            metadata: json!({}),
            replaces: None,
        };
        self.s
            .upload(&self.admin, vault, req, UploadLimits::default(), &mut text.as_bytes())
            .unwrap()
            .source
            .header
            .id
    }

    fn upload(&self, text: &str) -> ObjectId {
        self.upload_to(&self.v, text)
    }

    fn head(&self) -> i64 {
        self.s.activity_marks(&self.v).unwrap().head.map_or(-1, |h| h as i64)
    }

    fn log_seqs(&self) -> Vec<i64> {
        let snap = self.s.engine().store().snapshot(&self.v).unwrap();
        let (mut out, mut after) = (vec![], None);
        loop {
            let page = snap.events(after).unwrap();
            let Some(last) = page.last() else { return out };
            after = Some(last.seq);
            out.extend(page.iter().map(|e| e.seq as i64));
        }
    }
}

fn durable(frames: &[Frame]) -> Vec<i64> {
    frames.iter().filter(|f| f.event == "durable").map(seq_of).collect()
}

fn read_up_to(sse: &mut Sse, seq: i64) -> Vec<Frame> {
    let deadline = Instant::now() + WAIT;
    let mut out = vec![];
    loop {
        if durable(&out).last().is_some_and(|s| *s >= seq) {
            return out;
        }
        match sse.next(deadline.saturating_duration_since(Instant::now())) {
            Some(f) => out.push(f),
            None => panic!(
                "stopped before {seq}, with {:?}",
                out.iter().map(|f| (f.event.clone(), f.id.clone())).collect::<Vec<_>>()
            ),
        }
    }
}

#[test]
fn a_stream_says_hello_then_follows_the_log() {
    let w = world(fast());
    w.upload("one");
    let mut sse = w.follow(&w.root);
    assert_eq!(sse.status, 200);
    let lower = sse.headers.to_ascii_lowercase();
    assert!(lower.contains("content-type: text/event-stream"), "{lower}");
    assert!(lower.contains("cache-control: no-store"), "{lower}");
    assert!(lower.contains("x-request-id:"), "{lower}");
    let hello = sse.next(WAIT).unwrap();
    assert_eq!(hello.event, "hello");
    assert_eq!(hello.json()["head"], json!(w.head()));
    assert_eq!(
        hello.id.as_deref(),
        Some(w.head().to_string().as_str()),
        "so a client that hears nothing else can resume from here"
    );
    let source = w.upload("two");
    let frame = sse.next(WAIT).unwrap();
    assert_eq!(frame.event, "durable");
    let body = frame.json();
    assert_eq!(body["kind"], "state.recorded");
    assert_eq!(body["sources"][0]["id"], source.0);
    assert_eq!(body["jobs"][0]["kind"], "rendition");
    assert_eq!(body["jobs"][0]["state"], "queued");
    assert_eq!(body["jobs"][0]["source_id"], source.0);
    assert_eq!(body["counts"]["source"], 1);
}

#[test]
fn resuming_misses_nothing_and_repeats_nothing_from_the_ring_or_the_log() {
    // A ring of 4 means anything further back comes from the log.
    let w = world(ActivityLimits { ring_frames: 4, ..fast() });
    let mut first = w.follow(&w.root);
    first.next(WAIT).unwrap();
    for n in 0..20 {
        w.upload(&format!("doc {n}"));
    }
    let seen = read_up_to(&mut first, w.head());
    let seqs = durable(&seen);
    assert_eq!(seqs, w.log_seqs());
    for resume_after in [seqs[18], seqs[3], -1] {
        for name in ["last-event-id", "query"] {
            let id = resume_after.to_string();
            let mut again = if name == "query" {
                open(w.addr, &format!("{}?after={id}", w.path()), &[("Authorization", &format!("Bearer {}", w.root))])
            } else {
                w.follow_with(&w.root, &[("Last-Event-ID", &id)])
            };
            let frames = read_up_to(&mut again, w.head());
            assert_eq!(frames[0].event, "hello");
            assert_eq!(frames[0].json()["resumed"], json!(true));
            assert!(frames[0].id.is_none(), "a resuming hello mustn't move its position");
            let want: Vec<i64> = w.log_seqs().into_iter().filter(|s| *s > resume_after).collect();
            assert_eq!(durable(&frames), want, "after {resume_after} by {name}");
            w.upload(&format!("more {resume_after} {name}"));
            let later = again.next(WAIT).unwrap();
            assert_eq!(seq_of(&later), want.last().unwrap() + 1, "live frames carry straight on");
            read_up_to(&mut first, w.head());
        }
    }
    // The header comes first when both are there, as a browser reconnecting keeps its URL.
    let mut both = open(
        w.addr,
        &format!("{}?after=-1", w.path()),
        &[("Authorization", &format!("Bearer {}", w.root)), ("Last-Event-ID", &w.head().to_string())],
    );
    assert_eq!(both.next(WAIT).unwrap().json()["resumed"], json!(true));
    let n = w.head() + 1;
    w.upload("after both");
    assert_eq!(seq_of(&both.next(WAIT).unwrap()), n);
}

#[test]
fn a_catch_up_and_live_frames_arriving_during_it_come_in_order() {
    let w = world(ActivityLimits { ring_frames: 2, ..fast() });
    let mut keep_alive = w.follow(&w.root);
    keep_alive.next(WAIT).unwrap();
    for n in 0..1500 {
        w.upload(&format!("doc {n}"));
    }
    read_up_to(&mut keep_alive, w.head());
    let mut catching = w.follow_with(&w.root, &[("Last-Event-ID", "-1")]);
    for n in 0..30 {
        w.upload(&format!("during {n}"));
    }
    let frames = read_up_to(&mut catching, w.head());
    assert_eq!(durable(&frames), w.log_seqs(), "a page's worth and more, then live, with no gap or repeat");
}

#[test]
fn a_bad_position_gets_a_resync_and_not_an_error() {
    let w = world(fast());
    w.upload("one");
    for bad in ["999999", "not-a-number", "-5", "1.5", ""] {
        let head = w.head();
        let mut sse = w.follow_with(&w.root, &[("Last-Event-ID", bad)]);
        assert_eq!(sse.status, 200, "{bad}");
        let frames = sse.until("resync", WAIT);
        let resync = frames.last().unwrap();
        assert_eq!(resync.json()["reason"], "unknown_position", "{bad}");
        assert_eq!(resync.json()["head"], json!(head));
        assert_eq!(seq_of(resync), head, "the client carries on from here");
        w.upload(&format!("next {bad}"));
        assert_eq!(seq_of(&sse.next(WAIT).unwrap()), head + 1, "it follows on after the resync");
    }
    for bad in ["999999", "not-a-number", "-5", "1.5"] {
        let mut sse = open(
            w.addr,
            &format!("{}?after={bad}", w.path()),
            &[("Authorization", &format!("Bearer {}", w.root))],
        );
        assert_eq!(sse.status, 200, "{bad}");
        assert_eq!(sse.until("resync", WAIT).last().unwrap().json()["reason"], "unknown_position", "{bad}");
    }
}

#[test]
fn over_the_limit_is_refused_clearly_and_closing_one_makes_room() {
    let w = world(ActivityLimits { max_streams: 2, ..fast() });
    let (token, _, _) = w.reader("r");
    let mut one = w.follow(&token);
    assert_eq!(one.status, 200);
    let two = w.follow(&token);
    assert_eq!(two.status, 200);
    let mut three = w.follow(&token);
    assert_eq!(three.status, 429);
    let body: Value = serde_json::from_str(&three.plain_body()).unwrap();
    assert_eq!(body["error"]["code"], "too_many_streams");
    let message = body["error"]["message"].as_str().unwrap();
    assert!(message.contains("2 activity streams") && message.contains("limit"), "{message}");
    // Someone else's streams don't count against this one's.
    assert_eq!(w.follow(&w.root).status, 200);
    one.next(WAIT).unwrap();
    drop(one);
    let deadline = Instant::now() + WAIT;
    loop {
        let again = w.follow(&token);
        if again.status == 200 {
            break;
        }
        assert!(Instant::now() < deadline, "closing a stream never gave its place back");
        std::thread::sleep(Duration::from_millis(20));
    }
    drop(two);
}

#[test]
fn a_stream_for_one_vault_never_carries_another_vaults_events() {
    let w = world(fast());
    let other = w.s.create_vault(&w.admin, "other").unwrap().id;
    let (token, _, _) = w.reader("r");
    let mut sse = w.follow(&token);
    sse.next(WAIT).unwrap();
    let mut other_sources = BTreeSet::new();
    let mut mine = BTreeSet::new();
    for n in 0..20 {
        other_sources.insert(w.upload_to(&other, &format!("other {n}")).0);
        mine.insert(w.upload(&format!("mine {n}")).0);
    }
    let frames = read_up_to(&mut sse, w.head());
    let got: BTreeSet<String> = frames
        .iter()
        .filter(|f| f.event == "durable")
        .flat_map(|f| f.json()["sources"].as_array().cloned().unwrap_or_default())
        .map(|s| s["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(got, mine);
    assert!(got.is_disjoint(&other_sources));
    for id in &other_sources {
        assert!(!String::from_utf8_lossy(&sse.recorded).contains(id.as_str()));
    }
    // And it can't open one on the other vault, which it can't even tell exists.
    let mut denied = open(
        w.addr,
        &format!("/api/v1/vaults/{other}/activity"),
        &[("Authorization", &format!("Bearer {token}"))],
    );
    assert_eq!(denied.status, 404);
    assert!(denied.plain_body().contains("not_found"));
    let mut nobody = open(w.addr, &w.path(), &[]);
    assert_eq!(nobody.status, 401);
    assert!(nobody.plain_body().contains("unauthenticated") || nobody.status == 401);
}

fn ended_with(sse: &mut Sse) -> String {
    let frames = sse.until("end", WAIT);
    let end = frames.last().unwrap();
    let reason = end.json()["reason"].as_str().unwrap().to_string();
    assert!(sse.next(WAIT).is_none(), "nothing after an end frame");
    assert!(sse.closed, "the stream closes");
    reason
}

#[test]
fn revoking_a_token_during_a_stream_ends_it() {
    let w = world(fast());
    let (token, token_id, _) = w.reader("r");
    let mut sse = w.follow(&token);
    sse.next(WAIT).unwrap();
    w.upload("before");
    assert_eq!(sse.next(WAIT).unwrap().event, "durable");
    w.s.revoke_token(&w.admin, &token_id).unwrap();
    assert_eq!(ended_with(&mut sse), "auth_ended");
    assert_eq!(w.follow(&token).status, 401, "and it can't come back");
}

#[test]
fn disabling_a_principal_ends_its_streams() {
    let w = world(fast());
    let (token, _, principal) = w.reader("r");
    let (mut a, mut b) = (w.follow(&token), w.follow(&token));
    a.next(WAIT).unwrap();
    b.next(WAIT).unwrap();
    w.s.disable_principal(&w.admin, &principal).unwrap();
    assert_eq!(ended_with(&mut a), "auth_ended");
    assert_eq!(ended_with(&mut b), "auth_ended");
}

#[test]
fn a_token_running_out_during_a_stream_ends_it() {
    let w = world(fast());
    let p = w.s.create_principal(&w.admin, "e", PrincipalKind::Human, false).unwrap();
    let now = memex_core::Clock::now_ms(w.s.engine().clock());
    let t =
        w.s.create_token(
            &w.admin,
            NewToken {
                principal_id: Some(p.id),
                name: "t".into(),
                grants: vec![Grant {
                    vault: w.v.clone(),
                    scope: Scope::Read,
                }],
                expires_at_ms: Some(now + 60_000),
            },
        )
        .unwrap();
    let mut sse = w.follow(&t.token);
    sse.next(WAIT).unwrap();
    w.s.engine().clock().advance(Duration::from_secs(61));
    assert_eq!(ended_with(&mut sse), "auth_ended");
}

#[test]
fn ending_a_session_ends_its_stream_and_not_its_tokens_others() {
    let w = world(fast());
    let (token, _, _) = w.reader("r");
    let session = w.s.login(&token, "test").unwrap();
    let cookie = format!("memex_session={}", session.session_id);
    let mut by_cookie = open(w.addr, &w.path(), &[("Cookie", &cookie)]);
    assert_eq!(by_cookie.status, 200);
    by_cookie.next(WAIT).unwrap();
    let mut by_token = w.follow(&token);
    by_token.next(WAIT).unwrap();
    w.s.logout(&session.session_id).unwrap();
    assert_eq!(ended_with(&mut by_cookie), "auth_ended");
    w.upload("still going");
    let next = by_token.next(WAIT).unwrap();
    assert_eq!(next.event, "durable", "the token itself is still good");
}

#[test]
fn deleting_the_vault_ends_its_streams() {
    let w = world(fast());
    let mut sse = w.follow(&w.root);
    sse.next(WAIT).unwrap();
    w.s.delete_vault(&w.admin, &w.v).unwrap();
    let reason = ended_with(&mut sse);
    assert!(reason == "vault_gone" || reason == "scope_lost", "{reason}");
}

#[test]
fn overflow_drops_to_the_current_position_and_carries_on_with_no_gap() {
    let limits = ActivityLimits {
        buffer: BufferLimits {
            max_frames: 8,
            max_bytes: 1 << 20,
        },
        ..fast()
    };
    let s = new_server();
    s.set_activity_limits(limits);
    let root = s.bootstrap("root").unwrap().token;
    let admin = s.authenticate(&root).unwrap();
    let v = s.create_vault(&admin, "v").unwrap().id;
    let upload = |n: usize| {
        let req = UploadRequest {
            name: "d.txt".into(),
            trust_tier: "unverified".into(),
            metadata: json!({}),
            replaces: None,
        };
        s.upload(&admin, &v, req, UploadLimits::default(), &mut format!("doc {n}").as_bytes()).unwrap();
    };
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
    rt.block_on(async {
        use futures_util::StreamExt;
        let req = Request::builder()
            .uri(format!("/api/v1/vaults/{v}/activity"))
            .header("authorization", format!("Bearer {root}"))
            .body(Body::empty())
            .unwrap();
        let res = memex_http::router(s.clone()).oneshot(req).await.unwrap();
        for n in 0..40 {
            upload(n);
            // Past the buffer's size, in steps the reader sees apart.
            if n % 8 == 0 {
                tokio::time::sleep(Duration::from_millis(30)).await;
            }
        }
        tokio::time::sleep(Duration::from_millis(300)).await;
        let head = s.activity_marks(&v).unwrap().head.unwrap() as i64;
        let mut body = res.into_body().into_data_stream();
        let mut text = String::new();
        let mut frames = 0;
        while frames < 9 {
            let Ok(Some(Ok(chunk))) = tokio::time::timeout(Duration::from_millis(500), body.next()).await else {
                break;
            };
            text.push_str(std::str::from_utf8(&chunk).unwrap());
            frames += 1;
        }
        let blocks: Vec<&str> = text.split("\n\n").filter(|b| !b.is_empty()).collect();
        assert!(blocks.len() <= 8, "at most the buffer's worth was waiting: {}", blocks.len());
        let first = blocks.first().unwrap();
        assert!(first.contains("event: resync"), "{first}");
        assert!(first.contains("\"reason\":\"overflow\""), "{first}");
        let at: i64 = first.lines().next().unwrap().strip_prefix("id: ").unwrap().parse().unwrap();
        let rest: Vec<i64> = blocks[1..]
            .iter()
            .map(|b| b.lines().next().unwrap().strip_prefix("id: ").unwrap().parse().unwrap())
            .collect();
        assert_eq!(rest, ((at + 1)..=head).collect::<Vec<_>>(), "everything after the resync's position");
    });
}

#[test]
fn a_client_that_never_reads_is_disconnected_and_gives_back_its_place() {
    let w = world(ActivityLimits {
        max_streams: 1,
        stall_ms: 300,
        buffer: BufferLimits {
            max_frames: 16,
            max_bytes: 1 << 20,
        },
        ..fast()
    });
    let (token, _, _) = w.reader("r");
    let never_reads = w.follow(&token);
    assert_eq!(never_reads.status, 200);
    assert_eq!(w.follow(&token).status, 429, "it holds the only place");
    // Frames big enough to fill the socket's buffers in a reasonable number.
    let job = |n: usize| {
        json!({"put": {"job": {
            "header": {"id": format!("job{n}"), "creator": "c", "status": "active", "version": 1, "acl_id": null},
            "kind": "embed", "state": "queued", "input": {"docs": []}, "dedupe_key": format!("k{n}"), "max_attempts": 1, "next_run_at_ms": 0,
            "lease_owner": null, "lease_until_ms": null, "attempts": [], "created_at_ms": 0, "finished_at_ms": null
        }}})
    };
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut sent = 0;
    loop {
        for _ in 0..200 {
            let mutations: Vec<Value> = (0..32).map(|i| job(sent * 32 + i)).collect();
            w.s.apply_as(&w.admin, &w.v, "state.recorded", 1, json!({"mutations": mutations})).unwrap();
            sent += 1;
        }
        std::thread::sleep(Duration::from_millis(400));
        if w.follow(&token).status == 200 {
            break;
        }
        assert!(Instant::now() < deadline, "still connected after {sent} events");
    }
    drop(never_reads);
}

#[test]
fn shutdown_ends_open_streams_and_does_not_wait_for_them() {
    let mut w = world(fast());
    let mut sse = w.follow(&w.root);
    sse.next(WAIT).unwrap();
    w.stop.take().unwrap().send(()).unwrap();
    assert_eq!(ended_with(&mut sse), "server_stopping");
    let started = Instant::now();
    let done = w.server_done.take().unwrap();
    while !done.is_finished() {
        assert!(started.elapsed() < WAIT, "serve never returned");
        std::thread::sleep(Duration::from_millis(10));
    }
    done.join().unwrap();
}

#[test]
fn heartbeats_come_when_nothing_else_does() {
    let w = world(ActivityLimits { heartbeat_ms: 50, ..fast() });
    let mut sse = w.follow(&w.root);
    let frames = sse.until("heartbeat", WAIT);
    assert_eq!(frames.last().unwrap().json()["head"], json!(w.head()));
    assert_eq!(frames.last().unwrap().id, None, "a heartbeat doesn't move a client's position");
}

#[test]
fn worker_progress_reaches_a_stream_with_no_id() {
    let w = world(fast());
    let mut sse = w.follow(&w.root);
    sse.next(WAIT).unwrap();
    memex_core::Progress::report(
        w.s.as_ref(),
        &w.v,
        memex_types::ProgressUpdate {
            job_id: ObjectId("jobid".into()),
            source_id: Some(ObjectId("sourceid".into())),
            stage: memex_types::Stage::Ocr,
            done: 12,
            total: Some(400),
        },
    );
    let f = sse.until("progress", WAIT).pop().unwrap();
    assert_eq!(f.id, None, "progress has no resume");
    let body = f.json();
    assert_eq!(
        (body["stage"].as_str(), body["done"].as_u64(), body["total"].as_u64()),
        (Some("ocr"), Some(12), Some(400))
    );
    assert_eq!(body["job_id"], "jobid");
}

// ---- Stages on the board. ----

async fn board(s: &Arc<Server<Fakes>>, token: &str, v: &VaultId, query: &str) -> Value {
    let req = Request::builder()
        .uri(format!("/api/v1/vaults/{v}/activity/documents{query}"))
        .header("authorization", format!("Bearer {token}"))
        .body(Body::empty())
        .unwrap();
    let res = memex_http::router(s.clone()).oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    serde_json::from_slice(&to_bytes(res.into_body(), 10_000_000).await.unwrap()).unwrap()
}

#[tokio::test]
async fn the_board_follows_a_document_through_its_stages() {
    let s = new_server();
    let root = s.bootstrap("root").unwrap().token;
    let admin = s.authenticate(&root).unwrap();
    let v = s.create_vault(&admin, "v").unwrap().id;
    s.set_extraction(Some(ExtractionLimits::default()));
    let text = "Fartyget byggdes i Stockholm. Det är en mening om fartyget.";
    let req = UploadRequest {
        name: "a.txt".into(),
        trust_tier: "unverified".into(),
        metadata: json!({}),
        replaces: None,
    };
    let source = s
        .upload(&admin, &v, req, UploadLimits::default(), &mut text.as_bytes())
        .unwrap()
        .source
        .header
        .id;
    let stage_of = |b: &Value| {
        (
            b["items"][0]["stage"].as_str().unwrap().to_string(),
            b["items"][0]["status"].as_str().unwrap().to_string(),
        )
    };
    let b = board(&s, &root, &v, "").await;
    assert_eq!(stage_of(&b), ("stored".into(), "queued".into()));
    assert_eq!(b["items"][0]["source_id"], source.0);
    let job = s.claim_job(&v, RENDITION_JOB, "w").unwrap().unwrap();
    assert_eq!(stage_of(&board(&s, &root, &v, "").await), ("parsing".into(), "active".into()));
    memex_core::Progress::report(
        s.as_ref(),
        &v,
        memex_types::ProgressUpdate {
            job_id: job.header.id.clone(),
            source_id: Some(source.clone()),
            stage: memex_types::Stage::Ocr,
            done: 12,
            total: Some(400),
        },
    );
    let b = board(&s, &root, &v, "").await;
    assert_eq!(stage_of(&b), ("ocr".into(), "active".into()));
    assert_eq!(b["items"][0]["progress"]["done"], 12);
    memex_core::Progress::finished(s.as_ref(), &v, &job.header.id);
    let rows = s.build_rendition(&v, &source, plain(text)).unwrap();
    assert!(s.complete_job(&v, &job, rows).unwrap());
    assert_eq!(stage_of(&board(&s, &root, &v, "").await), ("indexing".into(), "active".into()));
    while s.project_lexical(&v).unwrap() == memex_core::StepOutcome::Progress {}
    while s.project_vector(&v).unwrap() == memex_core::StepOutcome::Progress {}
    assert_eq!(stage_of(&board(&s, &root, &v, "").await), ("embedding".into(), "queued".into()));
    let embed = s.claim_job(&v, EMBED_JOB, "w").unwrap().unwrap();
    assert_eq!(stage_of(&board(&s, &root, &v, "").await), ("embedding".into(), "active".into()));
    s.run_embed_job(&v, &embed).unwrap();
    assert!(s.complete_job(&v, &embed, vec![]).unwrap());
    while s.project_lexical(&v).unwrap() == memex_core::StepOutcome::Progress {}
    while s.project_vector(&v).unwrap() == memex_core::StepOutcome::Progress {}
    assert_eq!(stage_of(&board(&s, &root, &v, "").await), ("extraction".into(), "queued".into()));
    let extract = s.claim_job(&v, EXTRACT_JOB, "w").unwrap().unwrap();
    assert_eq!(stage_of(&board(&s, &root, &v, "").await), ("extraction".into(), "active".into()));
    let waited = s.wait_job(&v, &extract, "the LLM is away").unwrap();
    assert!(waited);
    let b = board(&s, &root, &v, "").await;
    assert_eq!(stage_of(&b), ("extraction".into(), "waiting".into()));
    assert!(b["items"][0]["waiting_since_ms"].is_i64());
    s.engine().clock().advance(Duration::from_secs(600));
    let extract = s.claim_job(&v, EXTRACT_JOB, "w").unwrap().unwrap();
    let segment = s
        .engine()
        .store()
        .snapshot(&v)
        .unwrap()
        .segments_of(&s.engine().store().snapshot(&v).unwrap().renditions_of(&source).unwrap()[0].header.id, None, 10)
        .unwrap();
    assert!(!segment.is_empty());
    let agent = ScriptedAgent::new(vec![reply(
        json!({"claims": [{"segment_id": "s1", "quote": "Fartyget byggdes i Stockholm.", "subject": "Fartyget", "predicate": "byggdes i", "value": "Stockholm"}], "entities": {}}),
    )]);
    let rows = s.run_extraction_job(&v, &extract, &agent).unwrap();
    assert!(s.complete_job(&v, &extract, rows).unwrap());
    while s.project_lexical(&v).unwrap() == memex_core::StepOutcome::Progress {}
    while s.project_vector(&v).unwrap() == memex_core::StepOutcome::Progress {}
    let b = board(&s, &root, &v, "").await;
    assert_eq!(stage_of(&b), ("review".into(), "active".into()));
    assert_eq!(b["items"][0]["pending_review"], 1);
    let by_stage = board(&s, &root, &v, "?stage=review").await;
    assert_eq!(by_stage["items"].as_array().unwrap().len(), 1);
    assert_eq!(board(&s, &root, &v, "?stage=done").await["items"].as_array().unwrap().len(), 0);
    assert_eq!(b["head"], json!(s.activity_marks(&v).unwrap().head.unwrap()));
}

#[tokio::test]
async fn the_board_shows_retrying_and_failed_jobs() {
    let s = new_server();
    let root = s.bootstrap("root").unwrap().token;
    let admin = s.authenticate(&root).unwrap();
    let v = s.create_vault(&admin, "v").unwrap().id;
    let req = UploadRequest {
        name: "a.txt".into(),
        trust_tier: "unverified".into(),
        metadata: json!({}),
        replaces: None,
    };
    s.upload(&admin, &v, req, UploadLimits::default(), &mut &b"words"[..]).unwrap();
    let job = s.claim_job(&v, RENDITION_JOB, "w").unwrap().unwrap();
    s.fail_job(&v, &job, "try again", true).unwrap();
    let b = board(&s, &root, &v, "").await;
    assert_eq!(
        (b["items"][0]["stage"].as_str(), b["items"][0]["status"].as_str()),
        (Some("parsing"), Some("retrying"))
    );
    assert_eq!(b["items"][0]["attempts"], 1);
    assert!(b["items"][0]["next_run_at_ms"].is_i64());
    s.engine().clock().advance(Duration::from_secs(600));
    let job = s.claim_job(&v, RENDITION_JOB, "w").unwrap().unwrap();
    s.fail_job(&v, &job, "no", false).unwrap();
    let b = board(&s, &root, &v, "").await;
    assert_eq!(b["items"][0]["status"], "failed");
}

#[tokio::test]
async fn the_board_pages_refuses_bad_input_and_counts_store_calls() {
    let s = new_server();
    let root = s.bootstrap("root").unwrap().token;
    let admin = s.authenticate(&root).unwrap();
    let v = s.create_vault(&admin, "v").unwrap().id;
    for n in 0..30 {
        let req = UploadRequest {
            name: "a.txt".into(),
            trust_tier: "unverified".into(),
            metadata: json!({}),
            replaces: None,
        };
        s.upload(&admin, &v, req, UploadLimits::default(), &mut format!("doc {n}").as_bytes()).unwrap();
    }
    let first = board(&s, &root, &v, "?limit=20").await;
    assert_eq!(first["items"].as_array().unwrap().len(), 20);
    let cursor = first["next_cursor"].as_str().unwrap().to_string();
    let second = board(&s, &root, &v, &format!("?limit=20&cursor={cursor}")).await;
    assert_eq!(second["items"].as_array().unwrap().len(), 10);
    assert!(second["next_cursor"].is_null());
    let reads = |query: &'static str| {
        let (s, root, v) = (s.clone(), root.clone(), v.clone());
        async move {
            let before = s.engine().store().counts().reads.load(std::sync::atomic::Ordering::Relaxed);
            board(&s, &root, &v, query).await;
            s.engine().store().counts().reads.load(std::sync::atomic::Ordering::Relaxed) - before
        }
    };
    let (few, many) = (reads("?limit=2").await, reads("?limit=25").await);
    assert_eq!(few, many, "the same store calls whatever the page's size");
    for bad in ["?limit=0", "?limit=201", "?stage=nope"] {
        let req = Request::builder()
            .uri(format!("/api/v1/vaults/{v}/activity/documents{bad}"))
            .header("authorization", format!("Bearer {root}"))
            .body(Body::empty())
            .unwrap();
        assert_eq!(
            memex_http::router(s.clone()).oneshot(req).await.unwrap().status(),
            StatusCode::BAD_REQUEST,
            "{bad}"
        );
    }
    let hidden = s.create_vault(&admin, "hidden").unwrap().id;
    let p = s.create_principal(&admin, "other", PrincipalKind::Human, false).unwrap();
    let t = s
        .create_token(
            &admin,
            NewToken {
                principal_id: Some(p.id),
                name: "t".into(),
                grants: vec![Grant {
                    vault: v.clone(),
                    scope: Scope::Read,
                }],
                expires_at_ms: None,
            },
        )
        .unwrap();
    let req = Request::builder()
        .uri(format!("/api/v1/vaults/{hidden}/activity/documents"))
        .header("authorization", format!("Bearer {}", t.token))
        .body(Body::empty())
        .unwrap();
    assert_eq!(memex_http::router(s.clone()).oneshot(req).await.unwrap().status(), StatusCode::NOT_FOUND);
}

// ---- No text. ----

fn scan_stream(w: &World) -> (Vec<u8>, Vec<Frame>) {
    let mut sse = w.follow_with(&w.root, &[("Last-Event-ID", "-1")]);
    assert_eq!(sse.status, 200);
    let frames = read_up_to(&mut sse, w.head());
    (sse.recorded.clone(), frames)
}

#[test]
fn a_marker_in_a_document_never_appears_in_any_frame() {
    let w = world(fast());
    let mut live = w.follow(&w.root);
    live.next(WAIT).unwrap();
    fill_with_marker(&w.s, &w.admin, &w.v, &|d| w.s.engine().clock().advance(d));
    // Something for progress frames too, whose type has no place for text.
    memex_core::Progress::report(
        w.s.as_ref(),
        &w.v,
        memex_types::ProgressUpdate {
            job_id: ObjectId("job1".into()),
            source_id: None,
            stage: memex_types::Stage::Extraction,
            done: 1,
            total: Some(2),
        },
    );
    let snap = w.s.engine().store().snapshot(&w.v).unwrap();
    let (mut events, mut after) = (vec![], None);
    loop {
        let page = snap.events(after).unwrap();
        let Some(last) = page.last() else { break };
        after = Some(last.seq);
        events.extend(page);
    }
    // First: the scan isn't empty-handed. The log is full of the marker.
    let with_marker = events.iter().filter(|e| e.payload.to_string().contains(MARKER)).count();
    assert!(with_marker >= 20, "the log should hold the marker: {with_marker} of {}", events.len());
    // Every kind of event there is got written.
    let kinds: BTreeSet<&str> = events.iter().map(|e| e.kind.as_str()).collect();
    let all: BTreeSet<&str> = memex_core::CURRENT_VERSIONS.iter().map(|(k, _)| *k).collect();
    assert_eq!(kinds, all, "every event kind on main");
    // And the rows that hold text.
    let mut tables = BTreeSet::new();
    let (live_frames, (replayed, replay_frames)) = (read_up_to(&mut live, w.head()), scan_stream(&w));
    for f in replay_frames.iter().filter(|f| f.event == "durable") {
        if let Some(counts) = f.json()["counts"].as_object() {
            tables.extend(counts.keys().cloned());
        }
    }
    for needed in [
        "source",
        "job",
        "rendition",
        "segment",
        "extraction",
        "derivation",
        "changeset",
        "assertion",
        "evidence",
        "page",
        "block",
        "slug",
        "revision",
        "edge",
        "entity",
        "entity_type",
        "review_verdict",
        "audit_run",
        "audit_check",
        "audit_state",
    ] {
        assert!(
            tables.contains(needed),
            "no frame came from a {needed} row, so this scan didn't cover it: {tables:?}"
        );
    }
    let statuses: BTreeSet<String> = replay_frames
        .iter()
        .filter(|f| f.event == "durable")
        .flat_map(|f| f.json()["jobs"].as_array().cloned().unwrap_or_default())
        .map(|j| j["status"].as_str().unwrap().to_string())
        .collect();
    for needed in ["queued", "active", "retrying", "waiting", "failed", "done"] {
        assert!(statuses.contains(needed), "no job was {needed}: {statuses:?}");
    }
    // The scan: every byte both ways, and however the marker might be spelt.
    assert_eq!(durable(&live_frames).len(), events.len());
    for bytes in [&replayed, &live.recorded] {
        let text = String::from_utf8_lossy(bytes).to_ascii_lowercase();
        assert!(!text.contains(&MARKER.to_ascii_lowercase()), "the marker is in a frame");
        assert!(text.len() > 1000, "recorded {} bytes", text.len());
    }
    eprintln!(
        "marker scan: {} events, {with_marker} holding the marker, {} bytes replayed and {} live, no marker in either; tables {tables:?}; not reached: {:?}",
        events.len(),
        replayed.len(),
        live.recorded.len(),
        Table::ALL
            .iter()
            .map(|t| format!("{t:?}"))
            .filter(|t| !tables.iter().any(|n| n.replace('_', "").eq_ignore_ascii_case(t)))
            .collect::<Vec<_>>()
    );
    // The frame function on each event agrees.
    for e in &events {
        assert!(
            !serde_json::to_string(&frame_of(e))
                .unwrap()
                .to_ascii_lowercase()
                .contains(&MARKER.to_ascii_lowercase())
        );
    }
}
