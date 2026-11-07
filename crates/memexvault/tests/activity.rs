//! The activity stream on real Postgres, through real HTTP. Tests named `pg_` run in process with no models, with `MEMEX_TEST_DATABASE_URL` set, as in AGENTS.md. Tests named `measure_` run the real binary, on a freshly created database, and also need `MEMEX_MODELS_DIR`. Timings are taken on a machine shared with other work, so each is printed with its load average.
//!
//! Pass rules, set before anything was measured:
//! - **Item 2.** 8 pairs of runs of the same ingest, alternating which arm goes first. The median over pairs of (documents/s with a client that never reads ÷ documents/s with none) must be at least 0.95. Every pair counts, whatever the load. A run with no resync or disconnect for that client doesn't count as having tested anything, and fails the test.
//! - **Item 5.** 6 pairs of 2,000 exact lookups over HTTP, alternating with 50 connected clients and with none. The median p95 with clients must be at most the 2 ms target. Also reported: the median over pairs of p95 with clients ÷ p95 without, which says what the streams add whatever the baseline is. RSS growth and peak connections are recorded. They have no limit in the spec, so they can't pass or fail.
// These tests read the environment, start the binary, and print what they measured.
#![allow(clippy::disallowed_methods, clippy::disallowed_macros)]
#[path = "../../memex-http/tests/support/marker_fixture.rs"]
mod marker_fixture;
#[path = "../../memex-http/tests/support/sse_client.rs"]
mod sse_client;

use marker_fixture::{MARKER, fill_with_marker, plain};
use memex_core::testing::{FakeEmbedder, FakeRasterizer, FakeReranker, ManualClock, MemBlobStore};
use memex_core::{
    ActivityLimits, Caller, Clock, NewToken, RENDITION_JOB, Registry, SearchEdges, Server, ServerEdges, Snapshot, Store, UploadLimits, UploadRequest,
};
use memex_lexical::TantivyLexical;
use memex_store_postgres::{PgStore, schema_of_vault};
use memex_types::{Audit, Grant, Principal, PrincipalKind, Scope, TokenRow, VaultId};
use memex_vector::PgVectors;
use memexvault::edges::{PgVectorIndex, SystemClock, SystemSeed, TantivyIndex};
use memexvault::pg_edge::PostgresStore;
use memexvault::registry_edge::PostgresRegistry;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sse_client::{Frame, Http, Sse, open, seq_of};
use std::collections::BTreeSet;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

const WAIT: Duration = Duration::from_secs(60);

struct Real<C>(std::marker::PhantomData<C>);

impl<C: Clock> ServerEdges for Real<C> {
    type Caches = memex_core::NoCaches;
    type Registry = PostgresRegistry;
    type Store = PostgresStore;
    type Blobs = MemBlobStore<ManualClock>;
    type Clock = C;
    type Seed = SystemSeed;
    type Rasterizer = FakeRasterizer;
    type Lexical = TantivyIndex;
    type Vector = PgVectorIndex;
    type Embedder = FakeEmbedder;
    type Reranker = FakeReranker;
}

fn database_url() -> String {
    std::env::var("MEMEX_TEST_DATABASE_URL").expect("not run: MEMEX_TEST_DATABASE_URL isn't set")
}

fn load() -> String {
    let out = Command::new("uptime")
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
        .unwrap_or_default();
    out.rsplit("load averages:").next().unwrap_or("").trim().to_string()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unique() -> String {
    format!("{}{:x}", std::process::id(), SystemClock::default().now_ms())
}

/// A server admin of this test's own, as earlier tests may already have used the database's one bootstrap.
fn root_of<C: Clock>(s: &Server<Real<C>>) -> (String, Caller) {
    let id = unique();
    let audit = Audit {
        actor: id.clone(),
        time_ms: 1,
        action: "principal.create".into(),
        target: id.clone(),
    };
    s.registry()
        .insert_principal(
            &Principal {
                id: id.clone(),
                name: format!("root-{id}"),
                kind: PrincipalKind::Human,
                server_admin: true,
                disabled: false,
                created_by: id.clone(),
                created_at_ms: 1,
            },
            &audit,
        )
        .unwrap();
    let raw = format!("mvx_{id}");
    s.registry()
        .insert_token(
            &TokenRow {
                id: format!("tok{id}"),
                principal_id: id.clone(),
                hash: hex(&Sha256::digest(raw.as_bytes())),
                name: "root".into(),
                grants: vec![],
                created_by: id.clone(),
                created_at_ms: 1,
                expires_at_ms: None,
                last_used_at_ms: None,
                revoked_at_ms: None,
            },
            &Audit {
                action: "token.create".into(),
                ..audit
            },
        )
        .unwrap();
    let caller = s.authenticate(&raw).unwrap();
    (raw, caller)
}

struct Rig<C: Clock + Send + Sync + 'static> {
    s: Arc<Server<Real<C>>>,
    root: String,
    admin: Caller,
    v: VaultId,
    addr: SocketAddr,
    dir: PathBuf,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
}

impl<C: Clock + Send + Sync + 'static> Drop for Rig<C> {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        let _ = self.s.delete_vault(&self.admin, &self.v);
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn rig<C>(name: &str, clock: C, limits: ActivityLimits) -> Rig<C>
where
    C: Clock + Send + Sync + 'static,
{
    let pg = PgStore::connect(&database_url(), 8).expect("connect");
    pg.migrate().expect("migrate");
    let pg = Arc::new(pg);
    let dir = std::env::temp_dir().join(format!("memex-activity-{name}-{}", unique()));
    std::fs::create_dir_all(&dir).unwrap();
    let s = Arc::new(Server::<Real<C>>::new(
        PostgresRegistry(pg.clone()),
        PostgresStore(pg.clone()),
        MemBlobStore::new(ManualClock::starting_at(0)),
        clock,
        SystemSeed,
        SearchEdges {
            lexical: TantivyIndex(TantivyLexical::new(&dir)),
            vector: PgVectorIndex(PgVectors::new(pg.clone())),
            embedder: FakeEmbedder::default(),
            reranker: FakeReranker::default(),
        },
    ));
    s.set_activity_limits(limits);
    let (root, admin) = root_of(&s);
    let v = s.create_vault(&admin, &format!("{name}-{}", unique())).unwrap().id;
    let probe = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = probe.local_addr().unwrap();
    drop(probe);
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let server = s.clone();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        rt.block_on(async {
            let shutdown = async {
                let _ = stopped.await;
            };
            memex_http::serve_with(server, axum::Router::new(), &addr.to_string(), shutdown).await.unwrap();
        });
    });
    for _ in 0..1000 {
        if std::net::TcpStream::connect(addr).is_ok() {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    Rig {
        s,
        root,
        admin,
        v,
        addr,
        dir,
        stop: Some(stop),
    }
}

fn fast() -> ActivityLimits {
    ActivityLimits {
        poll_ms: 20,
        recheck_ms: 100,
        heartbeat_ms: 60_000,
        ..ActivityLimits::default()
    }
}

impl<C: Clock + Send + Sync + 'static> Rig<C> {
    fn path(&self) -> String {
        format!("/api/v1/vaults/{}/activity", self.v)
    }

    fn follow(&self, token: &str, more: &[(&str, &str)]) -> Sse {
        let auth = format!("Bearer {token}");
        let mut headers = vec![("Authorization", auth.as_str())];
        headers.extend_from_slice(more);
        open(self.addr, &self.path(), &headers)
    }

    fn head(&self) -> i64 {
        self.s.activity_marks(&self.v).unwrap().head.map_or(-1, |h| h as i64)
    }

    /// One document all the way through parsing, as the workers would.
    fn ingest_one(&self, n: u64) {
        let text = format!("Dokument nummer {n} handlar om fartyget. Det byggdes i Stockholm.");
        let req = UploadRequest {
            name: format!("d{n}.txt"),
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
        let job = self.s.claim_job(&self.v, RENDITION_JOB, "w").unwrap().unwrap();
        let rows = self.s.build_rendition(&self.v, &source, plain(&text)).unwrap();
        assert!(self.s.complete_job(&self.v, &job, rows).unwrap());
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

    fn sql(&self, statement: &str) {
        let mut client = postgres::Client::connect(&database_url(), postgres::NoTls).unwrap();
        client.batch_execute(statement).unwrap();
    }

    fn reader(&self, name: &str) -> (String, String, String) {
        let p = self
            .s
            .create_principal(&self.admin, &format!("{name}-{}", unique()), PrincipalKind::Human, false)
            .unwrap();
        let t = self
            .s
            .create_token(
                &self.admin,
                NewToken {
                    principal_id: Some(p.id.clone()),
                    name: "t".into(),
                    grants: vec![Grant {
                        vault: self.v.clone(),
                        scope: Scope::Read,
                    }],
                    expires_at_ms: None,
                },
            )
            .unwrap();
        (t.token, t.row.id, p.id)
    }
}

fn durable(frames: &[Frame]) -> Vec<i64> {
    frames.iter().filter(|f| f.event == "durable").map(seq_of).collect()
}

/// Item 1: at random moments, hang up on a stream during ingest and resume from the last id seen.
#[test]
#[ignore]
fn pg_activity_random_disconnects_miss_and_repeat_nothing() {
    const ROUNDS: usize = 200;
    let r = Arc::new(rig("disconnects", SystemClock::default(), ActivityLimits { max_streams: 64, ..fast() }));
    let stop = Arc::new(AtomicBool::new(false));
    let ingest = {
        let (r, stop) = (r.clone(), stop.clone());
        std::thread::spawn(move || {
            let mut n = 0;
            while !stop.load(Ordering::SeqCst) {
                r.ingest_one(n);
                n += 1;
            }
            n
        })
    };
    let started = Instant::now();
    let mut rng: u64 = 0x38_38_38_38;
    let mut next = move |bound: u64| {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng % bound
    };
    let mut seen: Vec<i64> = vec![];
    let mut last = "-1".to_string();
    let (mut replayed_frames, mut empty_rounds, mut partial_cuts) = (0, 0, 0);
    for round in 0..ROUNDS {
        let mut sse = r.follow(&r.root, &[("Last-Event-ID", &last)]);
        assert_eq!(sse.status, 200, "round {round}");
        let reading_for = Duration::from_millis(5 + next(150));
        let until = Instant::now() + reading_for;
        let mut got = 0;
        while let Some(frame) = sse.next(until.saturating_duration_since(Instant::now())) {
            match frame.event.as_str() {
                "durable" => {
                    let seq = seq_of(&frame);
                    seen.push(seq);
                    last = seq.to_string();
                    got += 1;
                }
                "hello" | "heartbeat" | "progress" => {}
                other => panic!("round {round}: {other}: {}", frame.data),
            }
        }
        replayed_frames += got;
        if got == 0 {
            empty_rounds += 1;
        }
        // Some rounds go away part of the way through a chunk, as a dropped connection does. What wasn't a whole frame wasn't seen.
        if next(2) == 0 {
            let _ = sse.next(Duration::from_millis(1));
            partial_cuts += 1;
        }
        drop(sse);
    }
    stop.store(true, Ordering::SeqCst);
    let documents = ingest.join().unwrap();
    let mut sse = r.follow(&r.root, &[("Last-Event-ID", &last)]);
    let head = r.head();
    while seen.last().copied().unwrap_or(-1) < head {
        let frame = sse.next(WAIT).unwrap_or_else(|| panic!("stopped at {:?} before {head}", seen.last()));
        if frame.event == "durable" {
            seen.push(seq_of(&frame));
        }
    }
    let log = r.log_seqs();
    let (unique_seen, duplicates) = {
        let set: BTreeSet<i64> = seen.iter().copied().collect();
        (set.len(), seen.len() - set.len())
    };
    let missed = log.iter().filter(|s| !seen.contains(s)).count();
    println!(
        "item 1: {ROUNDS} disconnects in {:.1} s, {documents} documents, {} events in the log, {} seen once each, {missed} missed, {duplicates} duplicated, {empty_rounds} rounds heard nothing new, {partial_cuts} cut at random, {replayed_frames} frames before the last read; load {}",
        started.elapsed().as_secs_f64(),
        log.len(),
        unique_seen,
        load()
    );
    assert_eq!(seen, log, "exactly the log, in order");
    assert!(documents >= 5, "there was ingest going on: {documents}");
    assert_eq!((missed, duplicates), (0, 0));
}

fn ended_with(sse: &mut Sse, after: Instant) -> (String, Duration) {
    let frames = sse.until("end", WAIT);
    let reason = frames.last().unwrap().json()["reason"].as_str().unwrap().to_string();
    let took = after.elapsed();
    assert!(sse.next(WAIT).is_none(), "nothing after an end frame");
    assert!(sse.closed);
    (reason, took)
}

/// Item 3: what really happens when scope changes during a stream, on the real registry.
#[test]
#[ignore]
fn pg_activity_scope_changes_end_streams() {
    let limits = ActivityLimits { recheck_ms: 1_000, ..fast() };
    let r = rig("scope", SystemClock::default(), limits);
    let other = r.s.create_vault(&r.admin, &format!("other-{}", unique())).unwrap().id;
    let mut ends = vec![];

    // Revoking the token.
    let (token, token_id, _) = r.reader("revoked");
    let mut sse = r.follow(&token, &[]);
    assert_eq!(sse.status, 200);
    sse.next(WAIT).unwrap();
    r.ingest_one(1);
    assert_eq!(sse.next(WAIT).unwrap().event, "durable");
    let at = Instant::now();
    r.s.revoke_token(&r.admin, &token_id).unwrap();
    let (reason, took) = ended_with(&mut sse, at);
    assert_eq!(reason, "auth_ended");
    ends.push(("token revoked", took));
    assert_eq!(r.follow(&token, &[]).status, 401);

    // Ending a session, over HTTP, with the token behind it still good.
    let (token, _, _) = r.reader("session");
    let mut http = Http::new(r.addr, &token);
    let login = format!("{{\"token\":\"{token}\"}}");
    let head = format!(
        "POST /api/v1/session HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nx-memex-csrf: 1\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{login}",
        login.len()
    );
    let reply = {
        use std::io::{Read, Write};
        let mut socket = std::net::TcpStream::connect(r.addr).unwrap();
        socket.write_all(head.as_bytes()).unwrap();
        let mut text = String::new();
        socket.read_to_string(&mut text).unwrap();
        text
    };
    let cookie = reply
        .lines()
        .find_map(|l| l.strip_prefix("set-cookie: ").or_else(|| l.strip_prefix("Set-Cookie: ")))
        .and_then(|c| c.split(';').next())
        .unwrap_or_else(|| panic!("no cookie in {reply}"))
        .to_string();
    let mut by_cookie = open(r.addr, &r.path(), &[("Cookie", &cookie)]);
    assert_eq!(by_cookie.status, 200);
    by_cookie.next(WAIT).unwrap();
    let mut by_token = r.follow(&token, &[]);
    by_token.next(WAIT).unwrap();
    let logout =
        format!("DELETE /api/v1/session HTTP/1.1\r\nHost: 127.0.0.1\r\nCookie: {cookie}\r\nx-memex-csrf: 1\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    let at = Instant::now();
    {
        use std::io::{Read, Write};
        let mut socket = std::net::TcpStream::connect(r.addr).unwrap();
        socket.write_all(logout.as_bytes()).unwrap();
        let mut text = String::new();
        socket.read_to_string(&mut text).unwrap();
        assert!(text.starts_with("HTTP/1.1 204"), "{text}");
    }
    let (reason, took) = ended_with(&mut by_cookie, at);
    assert_eq!(reason, "auth_ended");
    ends.push(("session ended", took));
    r.ingest_one(2);
    let next = by_token.until("durable", WAIT).pop().unwrap();
    assert_eq!(next.event, "durable", "the token's own stream goes on");
    assert_eq!(http.json("GET", "/api/v1/whoami", None).0, 200, "and the token still works");

    // Disabling the principal, which takes its token's and sessions' streams with it.
    let (token, _, principal) = r.reader("disabled");
    let mut sse = r.follow(&token, &[]);
    sse.next(WAIT).unwrap();
    let at = Instant::now();
    r.s.disable_principal(&r.admin, &principal).unwrap();
    let (reason, took) = ended_with(&mut sse, at);
    assert_eq!(reason, "auth_ended");
    ends.push(("principal disabled", took));

    // The grant changing under an open stream. There's no route for this yet, so it's done in the table, which is what a later route would do. Losing this vault ends the stream, though the token is still good.
    let (token, token_id, _) = r.reader("narrowed");
    let mut sse = r.follow(&token, &[]);
    sse.next(WAIT).unwrap();
    let at = Instant::now();
    r.sql(&format!(
        "UPDATE memex_server.tokens SET grants = '[{{\"vault\":\"{}\",\"scope\":\"read\"}}]' WHERE id = '{token_id}'",
        other
    ));
    let (reason, took) = ended_with(&mut sse, at);
    assert_eq!(reason, "scope_lost");
    ends.push(("grant removed", took));
    assert_eq!(r.follow(&token, &[]).status, 404, "and it can't tell the vault is there");

    // A token reaching its expiry during a stream.
    let (token, token_id, _) = r.reader("expiring");
    let mut sse = r.follow(&token, &[]);
    sse.next(WAIT).unwrap();
    let at = Instant::now();
    r.sql(&format!(
        "UPDATE memex_server.tokens SET expires_at_ms = {} WHERE id = '{token_id}'",
        SystemClock::default().now_ms() + 500
    ));
    let (reason, took) = ended_with(&mut sse, at);
    assert_eq!(reason, "auth_ended");
    ends.push(("token expired (500 ms ahead)", took));

    // A stream on one vault hears nothing of another's.
    let (token, _, _) = r.reader("isolated");
    let mut sse = r.follow(&token, &[]);
    sse.next(WAIT).unwrap();
    let req = UploadRequest {
        name: "other.txt".into(),
        trust_tier: "unverified".into(),
        metadata: json!({}),
        replaces: None,
    };
    let foreign =
        r.s.upload(&r.admin, &other, req, UploadLimits::default(), &mut "the other vault".as_bytes())
            .unwrap()
            .source
            .header
            .id;
    r.ingest_one(3);
    let frames = sse.until("durable", WAIT);
    let mine: Vec<String> = frames.iter().map(|f| f.data.clone()).collect();
    assert!(!mine.iter().any(|d| d.contains(&foreign.0)), "{mine:?}");
    while let Some(f) = sse.next(Duration::from_millis(300)) {
        assert!(!f.data.contains(&foreign.0), "{}", f.data);
    }
    assert!(!String::from_utf8_lossy(&sse.recorded).contains(&foreign.0));

    // Deleting the vault itself.
    let mut sse = r.follow(&r.root, &[]);
    sse.next(WAIT).unwrap();
    let at = Instant::now();
    r.s.delete_vault(&r.admin, &r.v).unwrap();
    let (reason, took) = ended_with(&mut sse, at);
    assert!(reason == "vault_gone" || reason == "scope_lost", "{reason}");
    ends.push(("vault deleted", took));
    let _ = r.s.delete_vault(&r.admin, &other);
    println!("item 3: ends after the change, at a 1 s recheck: {ends:?}; load {}", load());
    for (what, took) in ends {
        // The check runs every second, so more than a few means it isn't the check that ended it, whatever the load.
        assert!(took < Duration::from_secs(30), "{what} took {took:?}");
    }
}

/// Item 6: spec 30 isn't built, so this simulates a log that no longer holds a position, by deleting rows from the events table.
#[test]
#[ignore]
fn pg_activity_a_position_the_log_no_longer_holds_gets_a_resync() {
    let r = rig("erased", SystemClock::default(), ActivityLimits { ring_frames: 2, ..fast() });
    for n in 0..12 {
        r.ingest_one(n);
    }
    let schema = schema_of_vault(&r.v);
    let head = r.head();
    // From here on, positions up to 10 aren't in the log.
    r.sql(&format!("DELETE FROM {schema}.events WHERE seq <= 10"));
    for position in ["3", "-1", "9"] {
        let mut sse = r.follow(&r.root, &[("Last-Event-ID", position)]);
        assert_eq!(sse.status, 200, "{position}");
        let frames = sse.until("resync", WAIT);
        assert_eq!(frames.last().unwrap().json()["reason"], "log_gap", "{position}");
        assert_eq!(seq_of(frames.last().unwrap()), r.head(), "{position}");
        assert!(durable(&frames).is_empty(), "{position}");
    }
    // Right at the edge of what's left, it carries on.
    let mut sse = r.follow(&r.root, &[("Last-Event-ID", "10")]);
    let want: Vec<i64> = (11..=head).collect();
    let mut got = vec![];
    while got.len() < want.len() {
        let f = sse.next(WAIT).expect("frames from the log's new start");
        if f.event == "durable" {
            got.push(seq_of(&f));
        }
    }
    assert_eq!(got, want);
    // Ahead of the log.
    let far = (head + 1000).to_string();
    assert_eq!(
        r.follow(&r.root, &[("Last-Event-ID", &far)]).until("resync", WAIT).last().unwrap().json()["reason"],
        "unknown_position"
    );

    // A gap opening right in front of a live stream, as erasure might leave it.
    let mut live = r.follow(&r.root, &[]);
    live.next(WAIT).unwrap();
    let ahead = r.head() + 5;
    r.sql(&format!(
        "INSERT INTO {schema}.events (seq, actor, time_ms, kind, schema_version, payload) VALUES ({ahead}, 'x', 1, 'state.recorded', 1, '{{}}')"
    ));
    let frames = live.until("resync", WAIT);
    assert_eq!(frames.last().unwrap().json()["reason"], "log_gap");
    assert_eq!(seq_of(frames.last().unwrap()), ahead, "and it carries on from the log's head");
    r.sql(&format!("DELETE FROM {schema}.events WHERE seq = {ahead}"));

    // What erasure is to do: a new cache generation makes every stream resync and empties the ring, so nothing is served from memory afterwards.
    let (r2_frames, r2_head) = {
        let r2 = rig("erased2", SystemClock::default(), fast());
        for n in 0..4 {
            r2.ingest_one(n);
        }
        let mut stream = r2.follow(&r2.root, &[]);
        stream.next(WAIT).unwrap();
        r2.ingest_one(99);
        stream.until("durable", WAIT);
        r2.s.bump_cache_generation(&r2.v).unwrap();
        let frames = stream.until("resync", WAIT);
        let reason = frames.last().unwrap().json()["reason"].as_str().unwrap().to_string();
        assert_eq!(reason, "erased");
        // A client resuming from before the erasure now reads the log, not what the ring held. The rows are still there, as this is only the generation changing.
        let mut after = r2.follow(&r2.root, &[("Last-Event-ID", "0")]);
        let fr = after.until("hello", WAIT);
        assert_eq!(fr.last().unwrap().json()["resumed"], json!(true));
        (frames.len(), r2.head())
    };
    println!(
        "item 6: gone and future positions resync; generation bump resync after {r2_frames} frames at head {r2_head}; load {}",
        load()
    );

    // Over the limit, with a clear error, on Postgres too.
    let limited = rig("limit", SystemClock::default(), ActivityLimits { max_streams: 1, ..fast() });
    let (token, _, _) = limited.reader("limited");
    let open_one = limited.follow(&token, &[]);
    assert_eq!(open_one.status, 200);
    let mut refused = limited.follow(&token, &[]);
    assert_eq!(refused.status, 429);
    let body: Value = serde_json::from_str(&refused.plain_body()).unwrap();
    assert_eq!(body["error"]["code"], "too_many_streams");
    println!("item 6: over the limit: {}", body["error"]["message"]);
}

/// Item 4 on Postgres, with the same fixture as the in-memory test, as what's stored there comes back through different code.
#[test]
#[ignore]
fn pg_activity_marker_scan() {
    let clock = ManualClock::starting_at(1_800_000_000_000);
    let r = rig("marker", clock, fast());
    let mut live = r.follow(&r.root, &[]);
    live.next(WAIT).unwrap();
    fill_with_marker(&r.s, &r.admin, &r.v, &|d| r.s.engine().clock().advance(d));
    let log = r.log_seqs();
    let snap = r.s.engine().store().snapshot(&r.v).unwrap();
    let (mut kinds, mut with_marker, mut after) = (BTreeSet::new(), 0, None);
    loop {
        let page = snap.events(after).unwrap();
        let Some(last) = page.last() else { break };
        after = Some(last.seq);
        for e in &page {
            kinds.insert(e.kind.clone());
            with_marker += usize::from(e.payload.to_string().contains(MARKER));
        }
    }
    drop(snap);
    let all: BTreeSet<String> = memex_core::CURRENT_VERSIONS.iter().map(|(k, _)| k.to_string()).collect();
    assert_eq!(kinds, all);
    assert!(with_marker >= 20, "{with_marker}");
    let mut replay = r.follow(&r.root, &[("Last-Event-ID", "-1")]);
    let mut seen = vec![];
    while seen.len() < log.len() {
        let f = replay.next(WAIT).expect("the whole log");
        if f.event == "durable" {
            seen.push(seq_of(&f));
        }
    }
    let mut live_seen = 0;
    while live_seen < log.len() {
        let f = live.next(WAIT).expect("the whole log, live");
        live_seen += usize::from(f.event == "durable");
    }
    assert_eq!(seen, log);
    for (name, bytes) in [("replay", &replay.recorded), ("live", &live.recorded)] {
        let text = String::from_utf8_lossy(bytes).to_ascii_lowercase();
        assert!(!text.contains(&MARKER.to_ascii_lowercase()), "the marker is in the {name} stream");
    }
    println!(
        "item 4 on Postgres: {} events, {with_marker} holding the marker, every kind of {}; {} bytes replayed, {} live, no marker in either",
        log.len(),
        all.len(),
        replay.recorded.len(),
        live.recorded.len()
    );
}

/// The board's one-statement reads against the same reads through the store's general calls, on Postgres.
#[test]
#[ignore]
fn pg_activity_board_reads_match_the_general_reads() {
    let r = rig("board", SystemClock::default(), fast());
    r.s.set_extraction(Some(memex_core::ExtractionLimits::default()));
    let mut ids = vec![];
    for n in 0..3 {
        r.ingest_one(n);
    }
    let req = UploadRequest {
        name: "waiting.txt".into(),
        trust_tier: "unverified".into(),
        metadata: json!({}),
        replaces: None,
    };
    let waiting =
        r.s.upload(&r.admin, &r.v, req, UploadLimits::default(), &mut "not parsed yet".as_bytes())
            .unwrap()
            .source
            .header
            .id;
    let snap = r.s.engine().store().snapshot(&r.v).unwrap();
    for row in snap.rows(memex_types::Table::Source, None).unwrap() {
        ids.push(row.header().id.clone());
    }
    let facts = snap.document_facts(&ids).unwrap();
    assert_eq!(facts.len(), 4);
    for (id, f) in ids.iter().zip(&facts) {
        assert_eq!(&f.source_id, id);
        let rendition = snap.renditions_of(id).unwrap().into_iter().next_back();
        assert_eq!(f.rendition, rendition, "{id}");
        if id != &waiting {
            let extraction = f.extraction.as_ref().expect("a queued extraction");
            assert_eq!(&f.extraction_job.as_ref().unwrap().header.id, &extraction.job_id);
        }
    }
    drop(snap);
    while r.s.project_lexical(&r.v).unwrap() == memex_core::StepOutcome::Progress {}
    while r.s.project_vector(&r.v).unwrap() == memex_core::StepOutcome::Progress {}
    let snap = r.s.engine().store().snapshot(&r.v).unwrap();
    let embeds = snap.open_embed_sources(100).unwrap();
    assert_eq!(embeds.iter().map(|(s, _)| s.clone()).collect::<BTreeSet<_>>().len(), 3, "{embeds:?}");
    assert!(embeds.iter().all(|(_, st)| *st == memex_types::JobState::Queued));
    let first = snap.events(None).unwrap().remove(0);
    assert_eq!(snap.event_time(first.seq).unwrap(), Some(first.time_ms));
    assert_eq!(snap.event_time(10_000_000).unwrap(), None);
    drop(snap);
    let mut http = Http::new(r.addr, &r.root);
    let (status, page) = http.json("GET", &format!("{}/documents?limit=10", r.path()), None);
    assert_eq!(status, 200, "{page}");
    assert_eq!(page["items"].as_array().unwrap().len(), 4);
    let stages: BTreeSet<String> = page["items"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["stage"].as_str().unwrap().to_string())
        .collect();
    assert!(stages.contains("stored") && stages.contains("embedding"), "{stages:?}");
    let (status, filtered) = http.json("GET", &format!("{}/documents?stage=stored", r.path()), None);
    assert_eq!(status, 200);
    assert_eq!(filtered["items"].as_array().unwrap().len(), 1);
    assert_eq!(filtered["items"][0]["source_id"], waiting.0);
}

// ---- Measurements, on the real binary. ----

struct Binary {
    child: Child,
    addr: SocketAddr,
    token: String,
    data: PathBuf,
}

impl Drop for Binary {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.data);
    }
}

fn binary(env: &[(&str, &str)]) -> Binary {
    let exe = env!("CARGO_BIN_EXE_memexvault");
    let url = database_url();
    let models = std::env::var("MEMEX_MODELS_DIR").expect("not run: MEMEX_MODELS_DIR isn't set");
    let port: u16 = std::env::var("MEMEX_ACTIVITY_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(18038);
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse().unwrap();
    let data = std::env::temp_dir().join(format!("memex-activity-bin-{}", unique()));
    std::fs::create_dir_all(&data).unwrap();
    let base = |c: &mut Command| {
        c.env("MEMEX_DATABASE_URL", &url)
            .env("MEMEX_MODELS_DIR", &models)
            .env("MEMEX_DATA_DIR", &data)
            .env("MEMEX_HTTP_BIND", addr.to_string())
            .env("MEMEX_EMBED_THREADS", "2");
        for (k, v) in env {
            c.env(k, v);
        }
    };
    let mut migrate = Command::new(exe);
    migrate.arg("migrate");
    base(&mut migrate);
    assert!(migrate.status().unwrap().success(), "migrate");
    let mut boot = Command::new(exe);
    boot.args(["bootstrap", "measure"]);
    base(&mut boot);
    let out = boot.output().unwrap();
    assert!(
        out.status.success(),
        "bootstrap needs a fresh database: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let token = String::from_utf8_lossy(&out.stdout).trim().to_string();
    let mut serve = Command::new(exe);
    serve.arg("serve").stdout(Stdio::null()).stderr(Stdio::null());
    base(&mut serve);
    let child = serve.spawn().unwrap();
    let deadline = Instant::now() + Duration::from_secs(120);
    while std::net::TcpStream::connect(addr).is_err() {
        assert!(Instant::now() < deadline, "the server never listened");
        std::thread::sleep(Duration::from_millis(100));
    }
    Binary { child, addr, token, data }
}

fn rss_kb(pid: u32) -> u64 {
    let out = Command::new("ps").args(["-o", "rss=", "-p", &pid.to_string()]).output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().parse().unwrap_or(0)
}

fn connections_now() -> i64 {
    let mut client = postgres::Client::connect(&database_url(), postgres::NoTls).unwrap();
    let db = database_url().rsplit('/').next().unwrap().to_string();
    let row = client
        .query_one("SELECT count(*) FROM pg_stat_activity WHERE datname = $1 AND pid <> pg_backend_pid()", &[&db])
        .unwrap();
    row.get(0)
}

/// Polls `pg_stat_activity` until told to stop, and gives back the highest count.
struct PeakConnections {
    stop: Arc<AtomicBool>,
    thread: std::thread::JoinHandle<i64>,
}

impl PeakConnections {
    fn start() -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = std::thread::spawn(move || {
            let mut client = postgres::Client::connect(&database_url(), postgres::NoTls).unwrap();
            let db = database_url().rsplit('/').next().unwrap().to_string();
            let mut peak = 0;
            while !flag.load(Ordering::SeqCst) {
                let row = client
                    .query_one("SELECT count(*) FROM pg_stat_activity WHERE datname = $1 AND pid <> pg_backend_pid()", &[&db])
                    .unwrap();
                peak = peak.max(row.get::<_, i64>(0));
                std::thread::sleep(Duration::from_millis(25));
            }
            peak
        });
        Self { stop, thread }
    }

    fn finish(self) -> i64 {
        self.stop.store(true, Ordering::SeqCst);
        self.thread.join().unwrap()
    }
}

fn median(mut values: Vec<f64>) -> f64 {
    values.sort_by(f64::total_cmp);
    let n = values.len();
    if n == 0 {
        return f64::NAN;
    }
    if n % 2 == 1 {
        values[n / 2]
    } else {
        (values[n / 2 - 1] + values[n / 2]) / 2.0
    }
}

fn new_vault(http: &mut Http, name: &str) -> String {
    let (status, v) = http.json("POST", "/api/v1/vaults", Some(json!({"name": name})));
    assert_eq!(status, 201, "{v}");
    v["id"].as_str().unwrap().to_string()
}

fn upload_http(http: &mut Http, vault: &str, name: &str, text: &str) {
    let path = format!("/api/v1/vaults/{vault}/sources?name={name}&trust_tier=unverified");
    let (status, body) = http.call("POST", &path, "text/plain", text.as_bytes());
    assert!(status == 201 || status == 200, "{status} {}", String::from_utf8_lossy(&body));
}

fn jobs_open(http: &mut Http, vault: &str) -> usize {
    let mut open = 0;
    for state in ["queued", "running"] {
        let (status, page) = http.json("GET", &format!("/api/v1/vaults/{vault}/jobs?state={state}&limit=200"), None);
        assert_eq!(status, 200, "{page}");
        open += page["items"].as_array().map_or(0, Vec::len);
    }
    open
}

/// One ingest: `documents` small uploads, and the time until nothing is unfinished.
fn ingest_run(http: &mut Http, vault: &str, documents: usize, tag: &str) -> f64 {
    let started = Instant::now();
    for n in 0..documents {
        let text = format!(
            "Document {tag} number {n} is about a ship.\n\nIt was built in Stockholm in {}.\n\nIt carried timber across the Baltic.",
            1800 + n
        );
        upload_http(http, vault, &format!("{tag}-{n}.txt"), &text);
    }
    let deadline = Instant::now() + Duration::from_secs(1800);
    while jobs_open(http, vault) > 0 {
        assert!(Instant::now() < deadline, "the ingest never finished");
        std::thread::sleep(Duration::from_millis(100));
    }
    documents as f64 / started.elapsed().as_secs_f64()
}

/// A client with a very small receive window that never reads.
fn never_reader(addr: SocketAddr, path: &str, token: &str) -> std::net::TcpStream {
    use std::io::Write;
    let socket = socket2::Socket::new(socket2::Domain::IPV4, socket2::Type::STREAM, None).unwrap();
    socket.set_recv_buffer_size(1024).unwrap();
    socket.connect(&addr.into()).unwrap();
    let mut stream: std::net::TcpStream = socket.into();
    let auth = format!("Bearer {token}");
    stream
        .write_all(sse_client::request_text(path, &[("Authorization", &auth)]).as_bytes())
        .unwrap();
    stream
}

/// What became of a client that never read: reading it now shows what the server last did about it.
fn fate_of(mut stream: std::net::TcpStream) -> (bool, bool, usize) {
    use std::io::Read;
    stream.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
    let (mut bytes, mut closed, mut buf) = (vec![], false, [0u8; 65536]);
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        match stream.read(&mut buf) {
            Ok(0) => {
                closed = true;
                break;
            }
            Ok(n) => bytes.extend_from_slice(&buf[..n]),
            Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => break,
            Err(_) => {
                closed = true;
                break;
            }
        }
    }
    let text = String::from_utf8_lossy(&bytes);
    (text.contains("event: resync"), closed, bytes.len())
}

/// Item 2. The pass rule is at the top of this file.
#[test]
#[ignore]
fn measure_ingest_with_a_client_that_never_reads() {
    const PAIRS: usize = 8;
    let documents: usize = std::env::var("MEMEX_ACTIVITY_DOCS").ok().and_then(|d| d.parse().ok()).unwrap_or(150);
    // A small buffer, so this client overflows within the run. The stall limit is longer than a run, so it stays connected for the whole of it.
    let server = binary(&[("MEMEX_ACTIVITY_BUFFER_FRAMES", "32"), ("MEMEX_ACTIVITY_STALL_S", "3600")]);
    let mut http = Http::new(server.addr, &server.token);
    let (mut ratios, mut resynced, mut disconnected) = (vec![], 0, 0);
    for pair in 0..PAIRS {
        let client_first = pair % 2 == 0;
        let (mut with, mut without) = (0.0, 0.0);
        for arm in 0..2 {
            let with_client = (arm == 0) == client_first;
            let tag = format!("p{pair}a{arm}");
            let vault = new_vault(&mut http, &format!("ingest-{tag}"));
            let client = with_client.then(|| never_reader(server.addr, &format!("/api/v1/vaults/{vault}/activity"), &server.token));
            if client.is_some() {
                std::thread::sleep(Duration::from_millis(300));
            }
            let rate = ingest_run(&mut http, &vault, documents, &tag);
            let rss = rss_kb(server.child.id());
            if with_client {
                with = rate;
                // Past the buffer's limit by now, if the socket filled up, so the reader's next pass has overflowed it.
                std::thread::sleep(Duration::from_millis(500));
                let (resync, closed, bytes) = fate_of(client.unwrap());
                resynced += usize::from(resync);
                disconnected += usize::from(closed);
                println!(
                    "pair {pair} with client: {rate:.2} docs/s, rss {rss} kB, resync {resync}, closed {closed}, read {bytes} bytes; load {}",
                    load()
                );
            } else {
                without = rate;
                println!("pair {pair} no client: {rate:.2} docs/s, rss {rss} kB; load {}", load());
            }
        }
        ratios.push(with / without);
        println!("pair {pair} ratio {:.3}", with / without);
    }
    let m = median(ratios.clone());
    println!(
        "item 2: ratios {ratios:.3?}, median {m:.3} against at least 0.95; {resynced} of {PAIRS} got a resync, {disconnected} were disconnected; load {}",
        load()
    );
    assert!(resynced + disconnected >= PAIRS, "a run where the client never overflowed tested nothing");
    assert!(m >= 0.95, "median ratio {m:.3}");
}

/// Item 5. The pass rule is at the top of this file. Connections are counted in the whole of this test's database.
#[test]
#[ignore]
fn measure_fifty_clients() {
    const PAIRS: usize = 6;
    const LOOKUPS: usize = 2000;
    const CLIENTS: usize = 50;
    const EXACT: &str = "0192f3a1-7c2e-7a10-9b3e-1234567890ab";
    let server = binary(&[("MEMEX_ACTIVITY_MAX_STREAMS", "64")]);
    let mut http = Http::new(server.addr, &server.token);
    let vault = new_vault(&mut http, "fifty");
    let stream_path = format!("/api/v1/vaults/{vault}/activity");
    let proposal = json!({
        "ops": [{"op": "add_assertion", "id": EXACT, "subject": "the ship", "predicate": "built in", "value": "Stockholm", "epistemic": "asserted", "machine": false, "confidence_permille": 1000, "valid_from_ms": null, "valid_to_ms": null, "evidence": []}],
        "rationale": "the row the exact lookups find",
        "idempotency_key": "exact-row"
    });
    let (status, made) = http.json("POST", &format!("/api/v1/vaults/{vault}/proposals"), Some(proposal));
    assert!(status < 300, "{status} {made}");
    let settle = Instant::now() + Duration::from_secs(120);
    while jobs_open(&mut http, &vault) > 0 && Instant::now() < settle {
        std::thread::sleep(Duration::from_millis(200));
    }
    let mut lookups = Http::new(server.addr, &server.token);
    let path = format!("/api/v1/vaults/{vault}/search?q={EXACT}");
    let (status, first) = lookups.json("GET", &path, None);
    assert_eq!(status, 200, "{first}");
    assert!(first.to_string().contains(EXACT), "the exact row is found: {first}");
    let mut measure = |n: usize| -> f64 {
        let mut times: Vec<f64> = Vec::with_capacity(n);
        for _ in 0..n {
            let t = Instant::now();
            let (status, _) = lookups.call("GET", &path, "text/plain", b"");
            times.push(t.elapsed().as_secs_f64() * 1000.0);
            assert_eq!(status, 200);
        }
        times.sort_by(f64::total_cmp);
        times[(times.len() * 95 / 100).min(times.len() - 1)]
    };
    // Warm up the paths, and find what's there before any stream.
    measure(200);
    std::thread::sleep(Duration::from_secs(2));
    let idle_connections = connections_now();
    let rss_before = rss_kb(server.child.id());
    let (mut with_p95, mut without_p95, mut ratios) = (vec![], vec![], vec![]);
    let (mut peak_with, mut peak_without, mut rss_peak_growth) = (0, 0, 0i64);
    for pair in 0..PAIRS {
        let clients_first = pair % 2 == 0;
        let (mut with, mut without) = (0.0, 0.0);
        for arm in 0..2 {
            let with_clients = (arm == 0) == clients_first;
            let peak = PeakConnections::start();
            let streams: Vec<Sse> = if with_clients {
                let auth = format!("Bearer {}", server.token);
                (0..CLIENTS).map(|_| open(server.addr, &stream_path, &[("Authorization", &auth)])).collect()
            } else {
                vec![]
            };
            for s in &streams {
                assert_eq!(s.status, 200);
            }
            let p95 = measure(LOOKUPS);
            let rss = rss_kb(server.child.id());
            let peak = peak.finish();
            if with_clients {
                with = p95;
                peak_with = peak_with.max(peak);
                rss_peak_growth = rss_peak_growth.max(rss as i64 - rss_before as i64);
                println!(
                    "pair {pair} with {CLIENTS}: p95 {p95:.2} ms, rss {rss} kB, peak connections {peak}; load {}",
                    load()
                );
            } else {
                without = p95;
                peak_without = peak_without.max(peak);
                println!(
                    "pair {pair} no clients: p95 {p95:.2} ms, rss {rss} kB, peak connections {peak}; load {}",
                    load()
                );
            }
            drop(streams);
            std::thread::sleep(Duration::from_millis(500));
        }
        with_p95.push(with);
        without_p95.push(without);
        ratios.push(with / without);
    }
    let per_client_kb = rss_peak_growth as f64 / CLIENTS as f64;
    let (m_with, m_without, m_ratio) = (median(with_p95.clone()), median(without_p95.clone()), median(ratios.clone()));
    println!(
        "item 5: p95 with {CLIENTS} clients {with_p95:.2?} median {m_with:.2} ms, without {without_p95:.2?} median {m_without:.2} ms, ratio {m_ratio:.3}; target at most 2 ms"
    );
    println!(
        "item 5: rss before {rss_before} kB, largest growth {rss_peak_growth} kB ({per_client_kb:.0} kB a client); connections idle {idle_connections}, peak with clients {peak_with}, peak without {peak_without}, so the streams added {}; load {}",
        peak_with - peak_without,
        load()
    );
    assert!(m_with <= 2.0, "median p95 {m_with:.2} ms with {CLIENTS} clients against 2 ms");
}
