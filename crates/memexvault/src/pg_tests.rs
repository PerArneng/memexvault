//! The Store on a real Postgres. Every test here is ignored, so a plain `cargo test` shows them as not run, never as passed.
//! Run with `MEMEX_TEST_DATABASE_URL` set: `cargo test -p memexvault --release -- --ignored --test-threads=1 pg_`
// These tests read the environment, measure time and print what they measured.
#![allow(clippy::disallowed_methods, clippy::disallowed_macros)]
use crate::pg_edge::PostgresStore;
use crate::writer::start_writer;
use memex_core::testing::{ManualClock, MemStore, contract};
use memex_core::{ApplyError, BatchLimits, Engine, Snapshot, Store, StoreError};
use memex_store_postgres::{LATEST_SERVER_VERSION, PgError, PgStore};
use memex_types::{EventDraft, ObjectId, RowKey, Table, VaultId};
use postgres::{Client, NoTls};
use serde_json::json;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

const LIMITS: BatchLimits = BatchLimits {
    max_events: 256,
    max_bytes: 4 << 20,
};

fn url() -> String {
    std::env::var("MEMEX_TEST_DATABASE_URL").expect("not run: MEMEX_TEST_DATABASE_URL isn't set")
}

fn store() -> PostgresStore {
    let pg = PgStore::connect(&url(), 8).expect("connect");
    pg.migrate().expect("migrate");
    PostgresStore(Arc::new(pg))
}

fn sql() -> Client {
    Client::connect(&url(), NoTls).expect("connect")
}

/// A vault name unique to this run, with its schema dropped first if it exists.
fn fresh(name: &str) -> VaultId {
    let s = store();
    let v = VaultId(format!("t-{name}"));
    s.0.drop_vault(&v).expect("drop");
    v
}

fn page(id: String) -> EventDraft {
    EventDraft {
        actor: "t".into(),
        kind: "page.created".into(),
        schema_version: 1,
        payload: json!({"id": id, "title": "T", "body": "B"}),
    }
}

fn pct(mut v: Vec<f64>, p: f64) -> f64 {
    v.sort_by(|a, b| a.total_cmp(b));
    v[((v.len() as f64 - 1.0) * p).round() as usize]
}

#[test]
#[ignore = "needs Postgres"]
fn pg_contract() {
    let s = store();
    for n in [
        "apply",
        "lease",
        "seq",
        "delete",
        "snap",
        "many",
        "page",
        "compact",
        "sections",
        "entity",
        "wiki",
        "batch",
        "iso-a",
        "iso-b",
        "never-opened",
    ] {
        s.0.drop_vault(&VaultId(format!("contract-{n}"))).expect("drop");
    }
    contract::run_all(&s, "contract-");
}

#[test]
#[ignore = "needs Postgres; about 3 minutes"]
fn pg_replay_of_100k_events_matches_mem_store() {
    let v = VaultId("random-source".into());
    let s = store();
    s.0.drop_vault(&v).expect("drop");
    let t = Instant::now();
    let (applied, refused, source, target) = contract::random_replay(MemStore::new(), &s, &v, 42, 100_000);
    println!(
        "replay: {applied} applied, {refused} refused by the reducer, {:.0} s, hash {source}",
        t.elapsed().as_secs_f64()
    );
    assert_eq!(source, target);
}

#[test]
#[ignore = "needs Postgres; 60 s per setting"]
fn pg_group_commit_throughput() {
    for (producers, secs) in [(1usize, 20u64), (16, 60), (64, 20)] {
        let v = fresh(&format!("throughput-{producers}"));
        let s = store();
        let lease = s.open_vault(&v).expect("open");
        let (handle, join) = start_writer(Engine::new(s, ManualClock::starting_at(0)), lease, LIMITS);
        let (stop, count) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicU64::new(0)));
        let t0 = Instant::now();
        let threads: Vec<_> = (0..producers)
            .map(|p| {
                let (h, stop, count) = (handle.clone(), stop.clone(), count.clone());
                std::thread::spawn(move || {
                    let (mut i, mut ms) = (0u64, vec![]);
                    while !stop.load(Ordering::Relaxed) {
                        let t = Instant::now();
                        h.submit(page(format!("p{p}-{i}"))).expect("submit");
                        ms.push(t.elapsed().as_secs_f64() * 1e3);
                        count.fetch_add(1, Ordering::Relaxed);
                        i += 1;
                    }
                    ms
                })
            })
            .collect();
        std::thread::sleep(Duration::from_secs(secs));
        stop.store(true, Ordering::Relaxed);
        let ms: Vec<f64> = threads.into_iter().flat_map(|t| t.join().expect("join")).collect();
        let (n, wall) = (count.load(Ordering::Relaxed), t0.elapsed().as_secs_f64());
        println!(
            "group commit: {producers} producers, {secs} s: {:.0} events/s, ack p50 {:.2} ms, p95 {:.2} ms",
            n as f64 / wall,
            pct(ms.clone(), 0.5),
            pct(ms, 0.95)
        );
        drop(handle);
        let engine = join.join().expect("join");
        let snap = engine.store().snapshot(&v).expect("snap");
        assert_eq!(snap.head().expect("head"), Some(n - 1));
        drop(snap);
        engine.store().0.drop_vault(&v).expect("drop");
    }
}

#[test]
#[ignore = "needs Postgres"]
fn pg_batched_lookups() {
    for rows in [50_000u64, 200_000] {
        let v = fresh(&format!("lookup-{rows}"));
        let s = store();
        let lease = s.open_vault(&v).expect("open");
        let engine = Engine::new(s, ManualClock::starting_at(0));
        let t = Instant::now();
        for chunk in (0..rows).collect::<Vec<u64>>().chunks(1000) {
            for r in engine.apply_batch(&lease, chunk.iter().map(|i| page(format!("{i:012}"))).collect()) {
                r.expect("apply");
            }
        }
        let load = t.elapsed().as_secs_f64();
        let mut rng = 1234567u64;
        let mut next = || {
            rng ^= rng << 13;
            rng ^= rng >> 7;
            rng ^= rng << 17;
            rng % rows
        };
        let snap = engine.store().snapshot(&v).expect("snap");
        let (mut batch, mut single) = (vec![], vec![]);
        for _ in 0..2000 {
            let keys: Vec<RowKey> = (0..50)
                .map(|_| RowKey {
                    table: Table::Page,
                    id: ObjectId(format!("{:012}", next())),
                })
                .collect();
            let t = Instant::now();
            assert_eq!(snap.get_many(&keys).expect("many").iter().flatten().count(), 50);
            batch.push(t.elapsed().as_secs_f64() * 1e3);
            let t = Instant::now();
            assert!(snap.get(&keys[0]).expect("get").is_some());
            single.push(t.elapsed().as_secs_f64() * 1e3);
        }
        drop(snap);
        println!(
            "lookups at {rows} rows: load {load:.0} s, batch of 50 p50 {:.2} ms p95 {:.2} ms, single p95 {:.2} ms",
            pct(batch.clone(), 0.5),
            pct(batch, 0.95),
            pct(single, 0.95)
        );
        engine.store().0.drop_vault(&v).expect("drop");
    }
}

#[test]
#[ignore = "needs Postgres"]
fn pg_registry_contract() {
    let s = store();
    let tag = format!(
        "c{:x}",
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("time").as_millis()
    );
    memex_core::testing::registry_contract::run_all(&crate::registry_edge::PostgresRegistry(s.0.clone()), &tag);
    let mut c = sql();
    c.execute("DELETE FROM memex_server.tokens WHERE id LIKE $1", &[&format!("{tag}%")])
        .expect("clean");
    c.execute("DELETE FROM memex_server.principals WHERE id LIKE $1", &[&format!("{tag}%")])
        .expect("clean");
    c.execute("DELETE FROM memex_server.vault_meta WHERE vault_id LIKE $1", &[&format!("{tag}%")])
        .expect("clean");
}

#[test]
#[ignore = "needs Postgres"]
fn pg_bad_cases() {
    let raw = PgStore::connect(&url(), 2).expect("connect");
    raw.migrate().expect("migrate");
    raw.check_server().expect("this server passes the startup checks");
    let s = store();
    let v = fresh("bad");

    // A second writer is refused, even from another connection.
    let lease = s.open_vault(&v).expect("open");
    assert!(matches!(s.open_vault(&v), Err(StoreError::WriterActive { .. })));

    // A lost connection stops the writer until it takes the lease again.
    let mut admin = sql();
    let killed = admin
        .query(
            "SELECT pg_terminate_backend(pid) FROM pg_locks WHERE locktype = 'advisory' AND pid <> pg_backend_pid()",
            &[],
        )
        .expect("terminate")
        .len();
    assert!(killed >= 1);
    std::thread::sleep(Duration::from_millis(200));
    let event = memex_types::Event {
        seq: 0,
        vault: v.clone(),
        actor: "t".into(),
        time_ms: 1,
        kind: "page.deleted".into(),
        schema_version: 1,
        payload: json!({}),
    };
    assert_eq!(s.commit(&lease, &event, &[]), Err(StoreError::LeaseLost));
    assert_eq!(s.commit(&lease, &event, &[]), Err(StoreError::LeaseLost));
    let again = s.open_vault(&v).expect("reopen after the lock was released");
    assert_eq!(s.commit(&again, &event, &[]).expect("commit").seq, 0);
    s.close_vault(&again).expect("close");

    // A rolled back batch leaves nothing, including when text can't be stored.
    let again = s.open_vault(&v).expect("open");
    let engine = Engine::new(store(), ManualClock::starting_at(0));
    drop(engine);
    let nul = memex_types::Event {
        seq: 1,
        vault: v.clone(),
        actor: "bad\0actor".into(),
        time_ms: 1,
        kind: "page.deleted".into(),
        schema_version: 1,
        payload: json!({}),
    };
    assert!(matches!(s.commit(&again, &nul, &[]), Err(StoreError::Backend(_))));
    assert_eq!(s.snapshot(&v).expect("snap").head().expect("head"), Some(0));
    s.close_vault(&again).expect("close");

    // An unknown schema version is refused.
    admin
        .execute("INSERT INTO memex_server.schema_version VALUES ($1)", &[&(LATEST_SERVER_VERSION + 1)])
        .expect("bump");
    assert!(matches!(raw.check_server(), Err(PgError::Refused(m)) if m.contains("needs")));
    assert!(matches!(raw.migrate(), Err(PgError::Refused(m)) if m.contains("newer")));
    admin
        .execute("DELETE FROM memex_server.schema_version WHERE version = $1", &[&(LATEST_SERVER_VERSION + 1)])
        .expect("restore");

    // synchronous_commit off is refused at startup.
    let database = url().rsplit('/').next().expect("a database in the URL").to_string();
    admin
        .batch_execute(&format!("ALTER DATABASE {database} SET synchronous_commit = off"))
        .expect("alter");
    let off = PgStore::connect(&url(), 2).expect("connect");
    assert!(matches!(off.check_server(), Err(PgError::Refused(m)) if m.contains("synchronous_commit")));
    admin
        .batch_execute(&format!("ALTER DATABASE {database} RESET synchronous_commit"))
        .expect("reset");
    PgStore::connect(&url(), 2).expect("connect").check_server().expect("passes again");

    // The password shows up in no error or debug output.
    let password = url()
        .split(':')
        .nth(2)
        .and_then(|p| p.split('@').next())
        .map(String::from)
        .expect("URL has a password");
    let bad_port = url().replace("55432", "1");
    let message = PgStore::connect(&bad_port, 1).expect_err("nothing listens there").to_string();
    assert!(!message.contains(&password), "{message}");
    assert!(!format!("{raw:?}").contains(&password));
    s.0.drop_vault(&v).expect("drop");
}

/// The crash victim: producers submit through the group writer and print each acknowledgement.
#[test]
#[ignore = "helper for the kill harness"]
fn pg_child() {
    let v = VaultId(std::env::var("MEMEX_KILL_VAULT").expect("vault"));
    let s = PostgresStore(Arc::new(PgStore::connect(&url(), 4).expect("connect")));
    let lease = s.open_vault(&v).expect("open");
    let (handle, _join) = start_writer(Engine::new(s, ManualClock::starting_at(0)), lease, LIMITS);
    let pid = std::process::id();
    println!("ready");
    let threads: Vec<_> = (0..4)
        .map(|t| {
            let h = handle.clone();
            std::thread::spawn(move || {
                let mut i = 0u64;
                loop {
                    match h.submit(page(format!("k{pid}-{t}-{i}"))) {
                        Ok(r) => {
                            let out = std::io::stdout();
                            let mut lock = out.lock();
                            let _ = writeln!(lock, "ack {}", r.seq);
                            let _ = lock.flush();
                        }
                        Err(ApplyError::Store(_)) => std::process::exit(3),
                        Err(e) => panic!("{e}"),
                    }
                    i += 1;
                }
            })
        })
        .collect();
    for t in threads {
        let _ = t.join();
    }
}

fn kill_harness(cycles: u32, kill_postgres: Option<String>) {
    let name = if kill_postgres.is_some() { "kill-pg" } else { "kill-app" };
    let v = fresh(name);
    store().open_vault(&v).map(|_| ()).expect("create");
    let me = std::env::current_exe().expect("exe");
    let mut rng = 0x9E37_79B9u64;
    let (mut lost, mut partial, mut gaps, mut reopen_failures) = (0, 0, 0, 0);
    let (mut highest, mut recover) = (-1i64, vec![]);
    let t0 = Instant::now();
    for cycle in 0..cycles {
        let mut child = Command::new(&me)
            .args(["--exact", "pg_tests::pg_child", "--ignored", "--nocapture", "--test-threads=1"])
            .env("MEMEX_KILL_VAULT", &v.0)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn");
        let out = BufReader::new(child.stdout.take().expect("stdout"));
        let last = Arc::new(AtomicU64::new(0));
        let writing = Arc::new(AtomicBool::new(false));
        let (l2, w2) = (last.clone(), writing.clone());
        let reader = std::thread::spawn(move || {
            for line in out.lines().map_while(Result::ok) {
                if let Some(seq) = line.split("ack ").nth(1).and_then(|s| s.trim().parse::<u64>().ok()) {
                    l2.fetch_max(seq + 1, Ordering::SeqCst);
                    w2.store(true, Ordering::SeqCst);
                }
            }
        });
        let wait = Instant::now();
        while !writing.load(Ordering::SeqCst) && wait.elapsed() < Duration::from_secs(30) {
            std::thread::sleep(Duration::from_millis(1));
        }
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        std::thread::sleep(Duration::from_millis(rng % 200));
        let t = Instant::now();
        if let Some(container) = &kill_postgres {
            Command::new("docker").args(["kill", "-s", "KILL", container]).output().expect("docker kill");
            let _ = child.kill();
            let _ = child.wait();
            Command::new("docker").args(["start", container]).output().expect("docker start");
        } else {
            child.kill().expect("kill");
            child.wait().expect("wait");
        }
        reader.join().expect("reader");
        // Wait for the server to accept connections, and for the dead writer's lock to clear.
        let mut c = loop {
            if let Ok(c) = Client::connect(&url(), NoTls) {
                break c;
            }
            assert!(t.elapsed() < Duration::from_secs(120), "postgres didn't come back");
            std::thread::sleep(Duration::from_millis(50));
        };
        let s = PostgresStore(Arc::new(PgStore::connect(&url(), 2).expect("connect")));
        let lease = loop {
            match s.open_vault(&v) {
                Ok(l) => break Some(l),
                Err(StoreError::WriterActive { .. }) if t.elapsed() < Duration::from_secs(30) => std::thread::sleep(Duration::from_millis(5)),
                Err(e) => {
                    eprintln!("cycle {cycle}: reopen: {e}");
                    break None;
                }
            }
        };
        if kill_postgres.is_some() {
            recover.push(t.elapsed().as_secs_f64());
        }
        let Some(lease) = lease else {
            reopen_failures += 1;
            continue;
        };
        let head = s.snapshot(&v).expect("snap").head().expect("head");
        let schema: String = c
            .query_one("SELECT schema_name FROM memex_server.vaults WHERE vault_id = $1", &[&v.0])
            .expect("schema")
            .get(0);
        let events: i64 = c.query_one(&format!("SELECT count(*) FROM {schema}.events"), &[]).expect("count").get(0);
        let pages: i64 = c.query_one(&format!("SELECT count(*) FROM {schema}.pages"), &[]).expect("count").get(0);
        let acked = last.load(Ordering::SeqCst) as i64 - 1;
        let head_i = head.map_or(-1, |h| h as i64);
        if head_i < acked {
            lost += 1;
            eprintln!("cycle {cycle}: acknowledged {acked}, head {head_i}");
        }
        if events != head_i + 1 {
            gaps += 1;
        }
        if pages != events {
            partial += 1;
        }
        highest = highest.max(acked);
        s.close_vault(&lease).expect("close");
        if cycle % 50 == 0 {
            eprintln!("cycle {cycle}: head {head_i}, {:.0} s", t0.elapsed().as_secs_f64());
        }
    }
    let p95 = if recover.is_empty() { 0.0 } else { pct(recover, 0.95) };
    println!(
        "kill harness {name}: {cycles} cycles in {:.1} min, highest acknowledged {highest}, lost {lost}, partial {partial}, gaps {gaps}, reopen failures {reopen_failures}, recovery p95 {p95:.1} s",
        t0.elapsed().as_secs_f64() / 60.0
    );
    assert_eq!((lost, partial, gaps, reopen_failures), (0, 0, 0, 0));
    assert!(highest > 0);
}

#[test]
#[ignore = "needs Postgres; about 8 minutes"]
fn pg_kill_memexvault_1000_times() {
    kill_harness(1000, None);
}

#[test]
#[ignore = "kills the Postgres container 100 times; about 10 minutes; needs MEMEX_TEST_CONTAINER"]
fn pg_kill_postgres_100_times() {
    kill_harness(100, Some(std::env::var("MEMEX_TEST_CONTAINER").expect("container name")));
}

/// A clock shifted forward by a set amount, so each restarted worker sees earlier leases as expired and backoffs as passed, with no waiting.
struct OffsetClock(i64);

impl memex_core::Clock for OffsetClock {
    fn now_ms(&self) -> i64 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("time").as_millis() as i64 + self.0
    }

    fn monotonic(&self) -> Duration {
        Duration::from_millis(self.now_ms().max(0) as u64)
    }
}

struct JobEdges;

impl memex_core::ServerEdges for JobEdges {
    type Registry = crate::registry_edge::PostgresRegistry;
    type Store = PostgresStore;
    type Blobs = crate::edges::FsBlobStore<memex_blob_fs::RealFs>;
    type Clock = OffsetClock;
    type Seed = crate::edges::SystemSeed;
    type Rasterizer = crate::rendition_job::ChildRasterizer;
    type Lexical = memex_core::testing::MemLexical;
    type Vector = memex_core::testing::MemVector;
    type Embedder = memex_core::testing::FakeEmbedder;
    type Reranker = memex_core::testing::FakeReranker;
}

fn job_server(data: &str, offset_ms: i64) -> memex_core::Server<JobEdges> {
    let pg = Arc::new(PgStore::connect(&url(), 8).expect("connect"));
    memex_core::Server::new(
        crate::registry_edge::PostgresRegistry(pg.clone()),
        PostgresStore(pg),
        crate::edges::FsBlobStore(memex_blob_fs::FsBlobs::new(data, memex_blob_fs::RealFs)),
        OffsetClock(offset_ms),
        crate::edges::SystemSeed,
        memex_core::testing::mem_search(),
    )
}

/// Each job writes one derived row with a fresh random id, so running it twice would really show as a duplicate. One in ten fails first, as work sometimes does.
struct DerivedOnce;

impl crate::workers::JobHandler for DerivedOnce {
    fn run(&self, _: &VaultId, job: &memex_types::JobRow) -> Result<Vec<memex_types::Mutation>, crate::workers::JobFailure> {
        use memex_core::Seed;
        let id = crate::edges::SystemSeed.next_u64();
        if id.is_multiple_of(10) {
            return Err(crate::workers::JobFailure {
                message: "transient".into(),
                retryable: true,
                waiting: false,
            });
        }
        let header = memex_types::Header {
            id: ObjectId(format!("derived-{id:016x}")),
            creator: "worker".into(),
            status: memex_types::Status::Active,
            version: 1,
            acl_id: None,
        };
        Ok(vec![memex_types::Mutation::Put(memex_types::Row::Page(memex_types::PageRow {
            header,
            title: job.header.id.0.clone(),
            body: "derived".into(),
            updated_at_ms: 0,
            slug: String::new(),
            redirects: vec![],
        }))])
    }
}

/// The crash victim for the job harness: a pool of workers, until it's killed.
#[test]
#[ignore = "helper for the job kill harness"]
fn pg_job_child() {
    let data = std::env::var("MEMEX_KILL_DATA").expect("data");
    let offset: i64 = std::env::var("MEMEX_KILL_OFFSET").expect("offset").parse().expect("offset");
    let server = Arc::new(job_server(&data, offset));
    let t = Instant::now();
    loop {
        match server.open_vaults() {
            Ok(_) => break,
            Err(_) if t.elapsed() < Duration::from_secs(30) => std::thread::sleep(Duration::from_millis(5)),
            Err(e) => panic!("{e}"),
        }
    }
    let stop = Arc::new(AtomicBool::new(false));
    let pools = vec![crate::workers::Pool {
        kind: memex_core::RENDITION_JOB.into(),
        workers: 4,
        handler: Arc::new(DerivedOnce),
    }];
    println!("ready");
    for t in crate::workers::start(server, pools, stop) {
        let _ = t.join();
    }
}

#[test]
#[ignore = "needs Postgres; a few minutes"]
fn pg_kill_jobs_200_times() {
    const JOBS: u64 = 1500;
    let data = std::env::temp_dir().join(format!("memex-jobs-{}", std::process::id()));
    let data_s = data.to_string_lossy().to_string();
    let mut admin = sql();
    store();
    admin
        .batch_execute("DELETE FROM memex_server.tokens; DELETE FROM memex_server.principals; DELETE FROM memex_server.vault_meta;")
        .expect("clean");
    let server = job_server(&data_s, 0);
    let token = server.bootstrap("harness").expect("bootstrap").token;
    let caller = server.authenticate(&token).expect("auth");
    let v = server.create_vault(&caller, "jobs").expect("vault").id;
    for i in 0..JOBS {
        let req = memex_core::UploadRequest {
            name: "d.txt".into(),
            trust_tier: "unverified".into(),
            metadata: json!({}),
            replaces: None,
        };
        server
            .upload(&caller, &v, req, memex_core::UploadLimits::default(), &mut format!("document {i}").as_bytes())
            .expect("upload");
    }
    let schema = memex_store_postgres::schema_for(&v);
    drop(server);
    let me = std::env::current_exe().expect("exe");
    let run = |offset: i64| {
        Command::new(&me)
            .args(["--exact", "pg_tests::pg_job_child", "--ignored", "--nocapture", "--test-threads=1"])
            .env("MEMEX_KILL_DATA", &data_s)
            .env("MEMEX_KILL_OFFSET", offset.to_string())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn")
    };
    let mut rng = 0xABCD_EF01u64;
    let t0 = Instant::now();
    let done_count = |c: &mut Client| -> i64 {
        c.query_one(&format!("SELECT count(*) FROM {schema}.jobs WHERE state = 'done'"), &[])
            .expect("count")
            .get(0)
    };
    let mut done_at_kill = vec![];
    for cycle in 0..200i64 {
        let mut child = run(cycle * 7_200_000);
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        std::thread::sleep(Duration::from_millis(30 + rng % 250));
        child.kill().expect("kill");
        child.wait().expect("wait");
        done_at_kill.push(done_count(&mut admin));
    }
    // A last run with no kill, to finish what's left.
    let mut last = run(200 * 7_200_000);
    let t = Instant::now();
    loop {
        let open: i64 = admin
            .query_one(&format!("SELECT count(*) FROM {schema}.jobs WHERE state IN ('queued', 'running')"), &[])
            .expect("count")
            .get(0);
        if open == 0 || t.elapsed() > Duration::from_secs(240) {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    last.kill().expect("kill");
    last.wait().expect("wait");
    let count = |c: &mut Client, sql: String| -> i64 { c.query_one(&sql, &[]).expect("q").get(0) };
    let done = count(&mut admin, format!("SELECT count(*) FROM {schema}.jobs WHERE state = 'done'"));
    let dead = count(&mut admin, format!("SELECT count(*) FROM {schema}.jobs WHERE state = 'dead'"));
    let open = count(&mut admin, format!("SELECT count(*) FROM {schema}.jobs WHERE state IN ('queued', 'running')"));
    let derived = count(&mut admin, format!("SELECT count(*) FROM {schema}.pages"));
    let duplicates = count(
        &mut admin,
        format!("SELECT count(*) FROM (SELECT title FROM {schema}.pages GROUP BY title HAVING count(*) > 1) d"),
    );
    let orphans = count(
        &mut admin,
        format!("SELECT count(*) FROM {schema}.pages p WHERE NOT EXISTS (SELECT 1 FROM {schema}.jobs j WHERE j.id = p.title AND j.state = 'done')"),
    );
    let without = count(
        &mut admin,
        format!("SELECT count(*) FROM {schema}.jobs j WHERE j.state = 'done' AND NOT EXISTS (SELECT 1 FROM {schema}.pages p WHERE p.title = j.id)"),
    );
    let retried = count(
        &mut admin,
        format!("SELECT count(*) FROM {schema}.jobs WHERE jsonb_array_length(row->'job'->'attempts') > 1"),
    );
    let partial_at_kill = done_at_kill.iter().filter(|d| **d > 0 && **d < JOBS as i64).count();
    println!(
        "job kill harness: 200 cycles in {:.1} min, {JOBS} jobs: done {done}, dead {dead}, open {open}, derived {derived}, duplicates {duplicates}, derived with no done job {orphans}, done with no derived {without}, jobs with more than one attempt {retried}, cycles that stopped mid-way {partial_at_kill}",
        t0.elapsed().as_secs_f64() / 60.0
    );
    assert_eq!((duplicates, orphans, without, open), (0, 0, 0, 0));
    assert_eq!(done + dead, JOBS as i64);
    assert_eq!(derived, done);
    assert!(partial_at_kill > 0, "the harness must really kill mid-way");
    admin
        .batch_execute("DELETE FROM memex_server.tokens; DELETE FROM memex_server.principals;")
        .expect("clean");
    let s = store();
    s.0.drop_vault(&v).expect("drop");
    admin
        .execute("DELETE FROM memex_server.vault_meta WHERE vault_id = $1", &[&v.0])
        .expect("clean");
    let _ = std::fs::remove_dir_all(data);
}

/// Spec 032 bad case: table data on a disk that fills up. The commit fails with an error, and what's already there stays readable.
#[test]
#[ignore = "needs a Postgres whose default tablespace is on a small disk, named by MEMEX_TEST_DATABASE_URL"]
fn pg_full_disk_returns_an_error_and_data_stays_readable() {
    let s = store();
    let v = fresh("full-disk");
    let lease = s.open_vault(&v).expect("open");
    let engine = Engine::new(s, ManualClock::starting_at(0));
    // Random hex, as Postgres would compress a repeated character down to nothing.
    let mut state = 88172645463325252u64;
    let body: String = (0..1 << 17)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            format!("{state:016x}")
        })
        .collect();
    let (mut written, mut failure) = (0u64, None);
    for i in 0..200 {
        let draft = EventDraft {
            actor: "t".into(),
            kind: "page.created".into(),
            schema_version: 1,
            payload: json!({"id": format!("p{i}"), "title": "T", "body": format!("{i}{body}")}),
        };
        match engine.apply(&lease, draft) {
            Ok(_) => written += 1,
            Err(e) => {
                failure = Some(e.to_string());
                break;
            }
        }
    }
    let failure = failure.expect("the disk should have filled up");
    println!("full disk: {written} commits, then: {failure}");
    assert!(failure.contains("53100") || failure.to_lowercase().contains("space"), "{failure}");
    let snap = engine.store().snapshot(&v).expect("reads still work");
    assert_eq!(snap.head().expect("head"), Some(written - 1));
    assert!(
        snap.get(&RowKey {
            table: Table::Page,
            id: ObjectId("p0".into())
        })
        .expect("get")
        .is_some()
    );
}
