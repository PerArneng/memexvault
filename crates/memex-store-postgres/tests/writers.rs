//! Spec 093: writers' lock connections against a real server. Ignored by default, as they need Postgres:
//! `MEMEX_TEST_DATABASE_URL=... cargo test -p memex-store-postgres --release --test writers -- --ignored --test-threads=1 --nocapture pg_writers`.
//! Each test makes vaults of its own, named after the process, and drops them at the end.
#![allow(clippy::disallowed_methods, clippy::disallowed_macros)]
use memex_store_postgres::{PgError, PgLease, PgStore, PoolLimits};
use memex_types::{Event, VaultId};
use postgres::{Client, NoTls};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

fn url() -> String {
    std::env::var("MEMEX_TEST_DATABASE_URL").expect("not run: MEMEX_TEST_DATABASE_URL isn't set")
}

/// The test URL with an application name of its own, so `pg_stat_activity` shows only this store's connections.
fn named(app: &str) -> String {
    let u = url();
    let sep = if u.contains('?') { '&' } else { '?' };
    format!("{u}{sep}application_name={app}")
}

fn unique(app: &str) -> String {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).expect("clock").as_nanos();
    format!("{app}_{}_{}", std::process::id(), nanos % 1_000_000_000)
}

fn admin() -> Client {
    Client::connect(&url(), NoTls).expect("admin connection")
}

/// This store's connections, and how many of them hold an advisory lock.
fn counts(admin: &mut Client, app: &str) -> (i64, i64) {
    let r = admin
        .query_one(
            "SELECT count(*), count(*) FILTER (WHERE EXISTS (SELECT 1 FROM pg_locks l WHERE l.pid = a.pid AND l.locktype = 'advisory')) FROM pg_stat_activity a WHERE application_name = $1",
            &[&app],
        )
        .expect("pg_stat_activity");
    (r.get(0), r.get(1))
}

/// Samples this store's connections every few milliseconds until told to stop: the most seen, the most holding a lock, and the samples taken.
fn sampler(app: String, stop: Arc<AtomicBool>) -> thread::JoinHandle<(i64, i64, usize)> {
    thread::spawn(move || {
        let mut admin = admin();
        let (mut most, mut most_locks, mut samples) = (0, 0, 0);
        while !stop.load(Ordering::Relaxed) {
            let (c, l) = counts(&mut admin, &app);
            most = most.max(c);
            most_locks = most_locks.max(l);
            samples += 1;
            thread::sleep(Duration::from_millis(5));
        }
        (most, most_locks, samples)
    })
}

fn event(vault: &VaultId, seq: u64) -> Event {
    Event {
        seq,
        vault: vault.clone(),
        actor: "t".into(),
        time_ms: seq as i64,
        kind: "page.deleted".into(),
        schema_version: 1,
        payload: serde_json::json!({}),
    }
}

fn pct(mut v: Vec<Duration>, p: f64) -> Duration {
    v.sort();
    v[((v.len() as f64 - 1.0) * p).round() as usize]
}

fn store(app: &str, max_open: usize) -> PgStore {
    let pg = PgStore::connect_with(
        &named(app),
        PoolLimits {
            max_open,
            idle: max_open,
            wait: Duration::from_millis(5000),
            writers: writers(),
        },
    )
    .expect("connect");
    pg.migrate().expect("migrate");
    pg
}

/// `MEMEX_WRITERS_LIMIT`, by default the store's default.
fn writers() -> usize {
    std::env::var("MEMEX_WRITERS_LIMIT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(memex_store_postgres::DEFAULT_WRITERS)
}

fn vaults(prefix: &str, n: usize) -> Vec<VaultId> {
    (0..n).map(|i| VaultId(format!("{prefix}-{i}"))).collect()
}

fn drop_all(pg: &PgStore, vs: &[VaultId]) {
    for v in vs {
        pg.drop_vault(v).expect("drop");
    }
}

/// The measurement the spec starts from, at 1, 10 and 100 vaults (`MEMEX_WRITERS_COUNTS`): the time to take a lease on a new vault and on one that exists, and connections while writing to every vault in turn. Asserts nothing about limits, so it runs on any version.
#[test]
#[ignore = "needs Postgres"]
fn pg_writers_measure() {
    let counts_env = std::env::var("MEMEX_WRITERS_COUNTS").unwrap_or_else(|_| "1,10,100".into());
    let rounds: usize = std::env::var("MEMEX_WRITERS_ROUNDS").ok().and_then(|s| s.parse().ok()).unwrap_or(20);
    for n in counts_env.split(',').map(|s| s.trim().parse::<usize>().expect("a count")) {
        let app = unique("writers_measure");
        let prefix = unique("wm");
        let vs = vaults(&prefix, n);
        // A new vault: the lock, its schema and its migrations.
        let first = store(&app, 10);
        let mut created = vec![];
        for v in &vs {
            let t = Instant::now();
            first.open_vault(v).expect("open a new vault");
            created.push(t.elapsed());
        }
        drop(first);
        // A vault that exists, as at startup: the lock and the schema check.
        let stop = Arc::new(AtomicBool::new(false));
        let watch = sampler(app.clone(), stop.clone());
        let pg = store(&app, 10);
        let mut opened = vec![];
        let mut leases: Vec<PgLease> = vec![];
        for v in &vs {
            let t = Instant::now();
            leases.push(pg.open_vault(v).expect("open an existing vault"));
            opened.push(t.elapsed());
        }
        let mut commits = vec![];
        for seq in 0..rounds as u64 {
            for (v, l) in vs.iter().zip(&leases) {
                let t = Instant::now();
                pg.commit_batch(l, &[(&event(v, seq), &[])]).expect("commit");
                commits.push(t.elapsed());
            }
        }
        let mut admin = admin();
        let (now_conns, now_locks) = counts(&mut admin, &app);
        stop.store(true, Ordering::Relaxed);
        let (most, most_locks, samples) = watch.join().expect("sampler");
        let load = std::fs::read_to_string("/proc/loadavg").unwrap_or_default();
        println!(
            "{n} vaults: new vault lease p50 {:?} p95 {:?}; existing vault lease p50 {:?} p95 {:?}; {} commits p50 {:?} p95 {:?}; connections at the end {now_conns} ({now_locks} holding locks), most seen {most} ({most_locks} holding locks) over {samples} samples; pool open {}; load {}",
            pct(created.clone(), 0.5),
            pct(created, 0.95),
            pct(opened.clone(), 0.5),
            pct(opened, 0.95),
            commits.len(),
            pct(commits.clone(), 0.5),
            pct(commits, 0.95),
            pg.pool_stats().open,
            load.split_whitespace().next().unwrap_or("?"),
        );
        drop_all(&pg, &vs);
    }
}

/// Item 1 and 2: a server with 200 vaults (`MEMEX_WRITERS_VAULTS`) holds no more than its pool limit plus its writer limit (`MEMEX_WRITERS_LIMIT`), sampled through a run that writes to all of them from several threads, and every write succeeds.
#[test]
#[ignore = "needs Postgres"]
fn pg_writers_200_vaults_stay_within_the_limit() {
    const POOL: usize = 10;
    const THREADS: usize = 16;
    let writers_limit = writers();
    let n: usize = std::env::var("MEMEX_WRITERS_VAULTS").ok().and_then(|s| s.parse().ok()).unwrap_or(200);
    let rounds: u64 = 10;
    let app = unique("writers_limit");
    let prefix = unique("wl");
    let vs = vaults(&prefix, n);
    let stop = Arc::new(AtomicBool::new(false));
    let watch = sampler(app.clone(), stop.clone());
    let pg = Arc::new(
        PgStore::connect_with(
            &named(&app),
            PoolLimits {
                max_open: POOL,
                idle: POOL,
                wait: Duration::from_millis(5000),
                writers: writers_limit,
            },
        )
        .expect("connect"),
    );
    pg.migrate().expect("migrate");
    let started = Instant::now();
    let leases: Vec<PgLease> = vs.iter().map(|v| pg.open_vault(v).expect("open")).collect();
    let opened = started.elapsed();
    // Readers on the pool as well, so both parts are busy at once.
    let reading = Arc::new(AtomicBool::new(true));
    let readers: Vec<_> = (0..THREADS)
        .map(|_| {
            let (pg, reading) = (pg.clone(), reading.clone());
            thread::spawn(move || {
                let mut reads = 0u64;
                while reading.load(Ordering::Relaxed) {
                    let mut c = pg.take_conn().expect("a pooled connection");
                    c.client.batch_execute("SELECT pg_sleep(0.002)").expect("query");
                    pg.give_conn(c);
                    reads += 1;
                }
                reads
            })
        })
        .collect();
    let timings = Arc::new(Mutex::new(vec![]));
    let started = Instant::now();
    let writers: Vec<_> = (0..THREADS)
        .map(|t| {
            let (pg, vs, leases, timings) = (pg.clone(), vs.clone(), leases.clone(), timings.clone());
            thread::spawn(move || {
                let mut mine = vec![];
                for seq in 0..rounds {
                    for i in (t..vs.len()).step_by(THREADS) {
                        let s = Instant::now();
                        pg.commit_batch(&leases[i], &[(&event(&vs[i], seq), &[])]).expect("every write succeeds");
                        mine.push(s.elapsed());
                    }
                }
                timings.lock().expect("lock").extend(mine);
            })
        })
        .collect();
    for w in writers {
        w.join().expect("writer");
    }
    let wrote = started.elapsed();
    reading.store(false, Ordering::Relaxed);
    let reads: u64 = readers.into_iter().map(|r| r.join().expect("reader")).sum();
    stop.store(true, Ordering::Relaxed);
    let (most, most_locks, samples) = watch.join().expect("sampler");
    let timings = timings.lock().expect("lock").clone();
    let mut admin = admin();
    for v in &vs {
        let head: Option<i64> = admin
            .query_one(&format!("SELECT max(seq) FROM {}.events", memex_store_postgres::schema_of_vault(v)), &[])
            .expect("head")
            .get(0);
        assert_eq!(head, Some(rounds as i64 - 1), "every write to {} is there", v.0);
    }
    let load = std::fs::read_to_string("/proc/loadavg").unwrap_or_default();
    println!(
        "{n} vaults, pool {POOL}, writers {writers_limit}: opened in {opened:?}; {} writes from {THREADS} threads in {wrote:?}, p50 {:?} p95 {:?} max {:?}; {reads} reads beside them; most connections {most}, most holding locks {most_locks}, over {samples} samples; load {}",
        timings.len(),
        pct(timings.clone(), 0.5),
        pct(timings.clone(), 0.95),
        pct(timings, 1.0),
        load.split_whitespace().next().unwrap_or("?"),
    );
    assert!(most as usize <= POOL + writers_limit, "{most} connections, past {POOL} + {writers_limit}");
    assert!(most_locks as usize <= writers_limit);
    drop_all(&pg, &vs);
}

/// Item 2: a write that can't get its writer connection waits the set time, then gets a clear error, and works once the connection is free. Item 3: a second store can't write to a vault while the first holds its lease.
#[test]
#[ignore = "needs Postgres"]
fn pg_writers_a_busy_writer_connection_times_out_and_fences() {
    let app = unique("writers_busy");
    let prefix = unique("wb");
    let vs = vaults(&prefix, 2);
    let pg = Arc::new(
        PgStore::connect_with(
            &named(&app),
            PoolLimits {
                max_open: 4,
                idle: 4,
                wait: Duration::from_millis(300),
                writers: 1,
            },
        )
        .expect("connect"),
    );
    pg.migrate().expect("migrate");
    let a = pg.open_vault(&vs[0]).expect("open a");
    let b = pg.open_vault(&vs[1]).expect("open b");
    // Another session locks a's events table, so a's commit holds the one writer connection until it's let go.
    let mut blocker = admin();
    let schema_a = memex_store_postgres::schema_of_vault(&vs[0]);
    blocker
        .batch_execute(&format!("BEGIN; LOCK TABLE {schema_a}.events IN ACCESS EXCLUSIVE MODE"))
        .expect("lock");
    let held = {
        let (pg, a, v) = (pg.clone(), a.clone(), vs[0].clone());
        thread::spawn(move || {
            let t = Instant::now();
            let r = pg.commit_batch(&a, &[(&event(&v, 0), &[])]);
            (r, t.elapsed())
        })
    };
    thread::sleep(Duration::from_millis(200));
    let t = Instant::now();
    let refused = pg.commit_batch(&b, &[(&event(&vs[1], 0), &[])]);
    let waited = t.elapsed();
    println!("b's write past the writer limit: {refused:?} after {waited:?}");
    match &refused {
        Err(PgError::Backend(m)) => assert!(m.contains("writer connection"), "{m}"),
        other => panic!("expected the writer-busy error, got {other:?}"),
    }
    assert!(waited >= Duration::from_millis(290) && waited < Duration::from_secs(2), "{waited:?}");
    blocker.batch_execute("COMMIT").expect("let go");
    let (r, took) = held.join().expect("a");
    assert_eq!(r.expect("a's commit, once let go"), 0);
    let t = Instant::now();
    assert_eq!(pg.commit_batch(&b, &[(&event(&vs[1], 0), &[])]).expect("b, once the connection is free"), 0);
    println!("a's commit took {took:?} while blocked; b's next write took {:?}", t.elapsed());

    // A second store on the same database is refused both vaults while the first holds them on one shared connection.
    let other = PgStore::connect(&url(), 2).expect("connect");
    for v in &vs {
        assert_eq!(other.open_vault(v), Err(PgError::WriterActive), "a second writer got {}", v.0);
    }
    // Closing one vault lets only that one go.
    pg.close_vault(&a).expect("close a");
    assert!(pg.lease_held(&b), "b's lease survives closing a on the same connection");
    let taken = other.open_vault(&vs[0]).expect("the other store takes a once it's let go");
    assert_eq!(other.open_vault(&vs[1]), Err(PgError::WriterActive));
    assert!(pg.commit_batch(&a, &[(&event(&vs[0], 1), &[])]).is_err(), "the closed lease can't write");
    other.close_vault(&taken).expect("close");
    drop(other);
    drop_all(&pg, &vs);
}

/// Losing a shared writer connection loses every lease on it, and each is taken again on a new connection.
#[test]
#[ignore = "needs Postgres"]
fn pg_writers_a_lost_shared_connection_loses_its_vaults_and_they_come_back() {
    let app = unique("writers_lost");
    let prefix = unique("wx");
    let vs = vaults(&prefix, 3);
    let pg = PgStore::connect_with(
        &named(&app),
        PoolLimits {
            max_open: 4,
            idle: 4,
            wait: Duration::from_millis(1000),
            writers: 1,
        },
    )
    .expect("connect");
    pg.migrate().expect("migrate");
    let leases: Vec<PgLease> = vs.iter().map(|v| pg.open_vault(v).expect("open")).collect();
    let mut admin = admin();
    let killed: i64 = admin
        .query_one(
            "SELECT count(pg_terminate_backend(a.pid)) FROM pg_stat_activity a WHERE application_name = $1 AND EXISTS (SELECT 1 FROM pg_locks l WHERE l.pid = a.pid AND l.locktype = 'advisory')",
            &[&app],
        )
        .expect("terminate")
        .get(0);
    assert_eq!(killed, 1, "one connection holds all three locks");
    // A write finds the connection gone: the first may get the server's own error before the client sees the connection close, as on a dedicated connection. Then none of the three holds.
    let first = pg.commit_batch(&leases[0], &[(&event(&vs[0], 0), &[])]);
    assert!(first.is_err(), "a write on a terminated connection");
    if first != Err(PgError::LeaseLost) {
        assert_eq!(
            pg.commit_batch(&leases[1], &[(&event(&vs[1], 0), &[])]),
            Err(PgError::LeaseLost),
            "after {first:?}"
        );
    }
    for l in &leases {
        assert!(!pg.lease_held(l), "{} still looks held", l.vault.0);
    }
    let again: Vec<PgLease> = vs.iter().map(|v| pg.open_vault(v).expect("take it again")).collect();
    for (v, l) in vs.iter().zip(&again) {
        assert_eq!(pg.commit_batch(l, &[(&event(v, 0), &[])]).expect("commit"), 0);
    }
    for l in &leases {
        assert!(!pg.lease_held(l), "an old lease came back");
    }
    // A commit refused inside its transaction leaves the shared connection clean for the next vault on it.
    assert!(matches!(
        pg.commit_batch(&again[0], &[(&event(&vs[0], 5), &[])]),
        Err(PgError::SequenceGap { .. })
    ));
    assert_eq!(pg.commit_batch(&again[1], &[(&event(&vs[1], 1), &[])]).expect("the next vault commits"), 1);
    let (conns, locks) = counts(&mut admin, &app);
    println!("after taking all three again: {conns} connections, {locks} holding locks");
    assert_eq!(locks, 1);
    drop_all(&pg, &vs);
}
