//! Spec 085: the pool's limit on open connections, against a real server. Ignored by default, as they need Postgres:
//! `MEMEX_TEST_DATABASE_URL=... cargo test -p memex-store-postgres --release --test pool -- --ignored --test-threads=1 --nocapture pg_pool`.
//! `pg_pool_survives_a_restart` also needs `MEMEX_TEST_CONTAINER`, a container it restarts, so never point it at a shared one.
#![allow(clippy::disallowed_methods, clippy::disallowed_macros)]
use memex_store_postgres::{PgError, PgStore, PoolLimits};
use memex_types::{Event, VaultId};
use postgres::{Client, NoTls};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
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

fn limits(max_open: usize, wait_ms: u64) -> PoolLimits {
    PoolLimits {
        max_open,
        idle: max_open,
        wait: Duration::from_millis(wait_ms),
        writers: memex_store_postgres::DEFAULT_WRITERS,
    }
}

fn admin() -> Client {
    Client::connect(&url(), NoTls).expect("admin connection")
}

fn count_named(admin: &mut Client, app: &str) -> i64 {
    admin
        .query_one("SELECT count(*) FROM pg_stat_activity WHERE application_name = $1", &[&app])
        .expect("pg_stat_activity")
        .get(0)
}

/// Samples `pg_stat_activity` for `app` every few milliseconds until told to stop, and returns the highest count and the number of samples.
fn sampler(app: String, stop: Arc<AtomicBool>) -> thread::JoinHandle<(i64, usize)> {
    thread::spawn(move || {
        let mut admin = admin();
        let (mut most, mut samples) = (0, 0);
        while !stop.load(Ordering::Relaxed) {
            most = most.max(count_named(&mut admin, &app));
            samples += 1;
            thread::sleep(Duration::from_millis(5));
        }
        (most, samples)
    })
}

fn is_pool_full(e: &PgError) -> bool {
    matches!(e, PgError::Backend(m) if m.contains("connection pool is full"))
}

/// A burst of readers, many more than the limit, never opens more than the limit, sampled through the run.
#[test]
#[ignore = "needs Postgres"]
fn pg_pool_never_opens_more_than_the_limit() {
    const LIMIT: usize = 6;
    const READERS: usize = 48;
    let app = unique("pool_limit");
    let pg = Arc::new(PgStore::connect_with(&named(&app), limits(LIMIT, 10_000)).expect("connect"));
    let stop = Arc::new(AtomicBool::new(false));
    let watch = sampler(app.clone(), stop.clone());
    let (done, full) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let most_seen_by_store = Arc::new(AtomicUsize::new(0));
    let started = Instant::now();
    let readers: Vec<_> = (0..READERS)
        .map(|_| {
            let (pg, done, full, most) = (pg.clone(), done.clone(), full.clone(), most_seen_by_store.clone());
            thread::spawn(move || {
                for _ in 0..20 {
                    match pg.take_conn() {
                        Ok(mut c) => {
                            most.fetch_max(pg.pool_stats().open, Ordering::Relaxed);
                            c.client.batch_execute("SELECT pg_sleep(0.01)").expect("query");
                            pg.give_conn(c);
                            done.fetch_add(1, Ordering::Relaxed);
                        }
                        Err(e) if is_pool_full(&e) => {
                            full.fetch_add(1, Ordering::Relaxed);
                        }
                        Err(e) => panic!("{e}"),
                    }
                }
            })
        })
        .collect();
    for r in readers {
        r.join().expect("reader");
    }
    let elapsed = started.elapsed();
    stop.store(true, Ordering::Relaxed);
    let (most, samples) = watch.join().expect("sampler");
    println!(
        "limit {LIMIT}, {READERS} readers: {} queries in {elapsed:?}, {} pool-full errors, most seen in pg_stat_activity {most} over {samples} samples, most the store counted {}",
        done.load(Ordering::Relaxed),
        full.load(Ordering::Relaxed),
        most_seen_by_store.load(Ordering::Relaxed)
    );
    assert!(samples >= 20, "the sampler ran {samples} times");
    assert!(most as usize <= LIMIT, "saw {most} connections with a limit of {LIMIT}");
    assert!(most >= 2, "the burst should have used more than one connection, and saw {most}");
    assert!(most_seen_by_store.load(Ordering::Relaxed) <= LIMIT);
    assert_eq!(done.load(Ordering::Relaxed), READERS * 20, "with a 10 s wait every reader gets a connection");
    let stats = pg.pool_stats();
    assert!(stats.open <= LIMIT && stats.idle == stats.open, "{stats:?}");
}

/// Past the limit a caller waits the set time and gets the pool-full error. A connection given back, or dropped, wakes a waiter.
#[test]
#[ignore = "needs Postgres"]
fn pg_pool_waits_then_fails_and_wakes_waiters() {
    let pg = Arc::new(PgStore::connect_with(&named(&unique("pool_wait")), limits(2, 300)).expect("connect"));
    let a = pg.take_conn().expect("first");
    let b = pg.take_conn().expect("second");

    let started = Instant::now();
    let err = pg.take_conn().err().expect("a third caller is refused");
    let waited = started.elapsed();
    println!("refused after {waited:?}: {err}");
    assert!(is_pool_full(&err), "{err}");
    assert!(waited >= Duration::from_millis(300), "gave up after {waited:?}, before the wait");
    assert!(waited < Duration::from_millis(1500), "took {waited:?}, well past the wait");

    // A longer wait, with a connection given back partway through.
    let pg = Arc::new(PgStore::connect_with(&named(&unique("pool_wake")), limits(2, 10_000)).expect("connect"));
    let a2 = pg.take_conn().expect("first");
    let b2 = pg.take_conn().expect("second");
    let waiter = {
        let pg = pg.clone();
        thread::spawn(move || {
            let started = Instant::now();
            let c = pg.take_conn().expect("the waiter gets the connection given back");
            (started.elapsed(), c)
        })
    };
    thread::sleep(Duration::from_millis(200));
    let given = Instant::now();
    pg.give_conn(a2);
    let (waited, kept) = waiter.join().expect("waiter");
    println!("given back: the waiter waited {waited:?}, {:?} after the give", given.elapsed());
    assert!(!kept.client.is_closed());
    assert_eq!(pg.pool_stats().open, 2);
    assert!(waited >= Duration::from_millis(150) && waited < Duration::from_secs(2), "{waited:?}");

    // A dropped connection, as a caller that failed drops its own, frees its place too.
    let waiter = {
        let pg = pg.clone();
        thread::spawn(move || {
            let started = Instant::now();
            pg.take_conn().map(|_| started.elapsed())
        })
    };
    thread::sleep(Duration::from_millis(200));
    drop(b2);
    let waited = waiter.join().expect("waiter").expect("the waiter gets a new connection");
    println!("dropped: the waiter waited {waited:?}");
    assert!(waited >= Duration::from_millis(150) && waited < Duration::from_secs(2), "{waited:?}");
    assert!(pg.pool_stats().open <= 2, "{:?}", pg.pool_stats());
    drop((a, b, kept));
}

/// Readers holding every pooled connection don't stop a writer opening its vault and committing.
#[test]
#[ignore = "needs Postgres"]
fn pg_pool_burst_does_not_starve_a_writer() {
    const LIMIT: usize = 3;
    let app = unique("pool_writer");
    let pg = Arc::new(PgStore::connect_with(&named(&app), limits(LIMIT, 200)).expect("connect"));
    pg.migrate().expect("migrate");
    let vault = VaultId(unique("pool_writer_vault"));
    let _ = pg.drop_vault(&vault);

    let stop = Arc::new(AtomicBool::new(false));
    let (held, refused) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let readers: Vec<_> = (0..24)
        .map(|_| {
            let (pg, stop, held, refused) = (pg.clone(), stop.clone(), held.clone(), refused.clone());
            thread::spawn(move || {
                while !stop.load(Ordering::Relaxed) {
                    match pg.take_conn() {
                        Ok(mut c) => {
                            held.fetch_add(1, Ordering::Relaxed);
                            c.client.batch_execute("SELECT pg_sleep(0.05)").expect("query");
                            pg.give_conn(c);
                        }
                        Err(e) if is_pool_full(&e) => {
                            refused.fetch_add(1, Ordering::Relaxed);
                        }
                        Err(e) => panic!("{e}"),
                    }
                }
            })
        })
        .collect();
    // Let the burst fill the pool first.
    let filled = Instant::now();
    while pg.pool_stats().open < LIMIT && filled.elapsed() < Duration::from_secs(5) {
        thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(pg.pool_stats().open, LIMIT, "the burst holds every pooled connection");

    let started = Instant::now();
    let lease = pg.open_vault(&vault).expect("the writer opens its vault during the burst");
    let opened = started.elapsed();
    let mut commits = vec![];
    for seq in 0..20u64 {
        let event = Event {
            seq,
            vault: vault.clone(),
            actor: "t".into(),
            time_ms: seq as i64,
            kind: "page.deleted".into(),
            schema_version: 1,
            payload: serde_json::json!({}),
        };
        let t = Instant::now();
        assert_eq!(pg.commit_batch(&lease, &[(&event, &[])]).expect("the writer commits during the burst"), seq);
        commits.push(t.elapsed());
    }
    let open_during = pg.pool_stats().open;
    stop.store(true, Ordering::Relaxed);
    for r in readers {
        r.join().expect("reader");
    }
    commits.sort();
    println!(
        "writer opened in {opened:?}, 20 commits: median {:?}, max {:?}; readers got {} connections and {} pool-full errors; pool open during the commits {open_during}",
        commits[10],
        commits[19],
        held.load(Ordering::Relaxed),
        refused.load(Ordering::Relaxed)
    );
    assert!(refused.load(Ordering::Relaxed) > 0, "the burst should have been past the limit");
    assert!(open_during <= LIMIT);
    let mut admin = admin();
    let (with_lock, total): (i64, i64) = {
        let r = admin
            .query_one(
                "SELECT count(*) FILTER (WHERE EXISTS (SELECT 1 FROM pg_locks l WHERE l.pid = a.pid AND l.locktype = 'advisory')), count(*) FROM pg_stat_activity a WHERE application_name = $1",
                &[&app],
            )
            .expect("count");
        (r.get(0), r.get(1))
    };
    println!("after the burst: {total} connections for this store, {with_lock} holding the writer's lock");
    assert_eq!(with_lock, 1);
    assert!(total as usize <= LIMIT + 1);
    pg.delete_vault(&lease).expect("clean up");
}

/// A server restart with callers waiting: nothing hangs, closed connections stop counting, and the pool works again without a new store.
#[test]
#[ignore = "needs Postgres and a container of its own to restart"]
fn pg_pool_survives_a_restart() {
    let container = std::env::var("MEMEX_TEST_CONTAINER").expect("not run: MEMEX_TEST_CONTAINER isn't set");
    let engine = std::env::var("MEMEX_TEST_CONTAINER_ENGINE").unwrap_or_else(|_| "docker".into());
    const LIMIT: usize = 4;
    let app = unique("pool_restart");
    let pg = Arc::new(PgStore::connect_with(&named(&app), limits(LIMIT, 20_000)).expect("connect"));

    // Two idle connections and two held, so both kinds are closed by the restart.
    let idle: Vec<_> = (0..2).map(|_| pg.take_conn().expect("take")).collect();
    for c in idle {
        pg.give_conn(c);
    }
    let held: Vec<_> = (0..LIMIT).map(|_| pg.take_conn().expect("take")).collect();
    assert_eq!(pg.pool_stats().open, LIMIT);

    let outcomes = Arc::new(Mutex::new(vec![]));
    let waiters: Vec<_> = (0..3)
        .map(|_| {
            let (pg, outcomes) = (pg.clone(), outcomes.clone());
            thread::spawn(move || {
                let started = Instant::now();
                let got = pg.take_conn().and_then(|mut c| {
                    c.client.query_one("SELECT 1", &[]).map(|_| ()).map_err(|e| PgError::Backend(e.to_string()))?;
                    pg.give_conn(c);
                    Ok(())
                });
                outcomes.lock().unwrap().push((started.elapsed(), got));
            })
        })
        .collect();
    thread::sleep(Duration::from_millis(200));

    let restarted = Instant::now();
    let status = std::process::Command::new(&engine).args(["restart", &container]).status().expect("restart");
    assert!(status.success(), "{engine} restart {container} failed");
    // Wait until the server accepts connections again.
    let up = Instant::now();
    while Client::connect(&url(), NoTls).is_err() {
        assert!(up.elapsed() < Duration::from_secs(60), "the server didn't come back");
        thread::sleep(Duration::from_millis(100));
    }
    println!("restart took {:?}", restarted.elapsed());

    // The held connections died with the server. Their callers fail and drop them, as any caller does on an error.
    let mut held = held;
    for c in &mut held {
        assert!(c.client.query_one("SELECT 1", &[]).is_err(), "a connection from before the restart still works");
    }
    drop(held);

    for w in waiters {
        w.join().expect("waiter");
    }
    let outcomes = outcomes.lock().unwrap();
    for (waited, got) in outcomes.iter() {
        println!("waiter: {waited:?} {got:?}");
        assert!(got.is_ok(), "a waiter failed after the server came back: {got:?}");
        assert!(*waited < Duration::from_secs(20), "{waited:?}");
    }
    let stats = pg.pool_stats();
    println!("after the restart: {stats:?}");
    assert!(stats.open <= LIMIT, "{stats:?}");

    // Every place is usable again: the limit's worth of connections at once, with none from before the restart counting.
    let fresh: Vec<_> = (0..LIMIT)
        .map(|_| {
            let mut c = pg.take_conn().expect("take after the restart");
            c.client.query_one("SELECT 1", &[]).expect("a working connection");
            c
        })
        .collect();
    assert_eq!(pg.pool_stats().open, LIMIT);
    let mut admin = admin();
    assert_eq!(count_named(&mut admin, &app) as usize, LIMIT);
    drop(fresh);
    assert_eq!(pg.pool_stats().open, 0);
}

/// The cost of the limit: short reads at several limits, and with none, at a load below the default and one above it. Each time includes the wait for a connection.
#[test]
#[ignore = "needs Postgres"]
fn pg_pool_latency_with_and_without_the_limit() {
    let run = |max_open: usize, threads: usize| {
        let app = unique("pool_latency");
        let pg = Arc::new(
            PgStore::connect_with(
                &named(&app),
                PoolLimits {
                    max_open,
                    idle: 16,
                    wait: Duration::from_secs(30),
                    writers: memex_store_postgres::DEFAULT_WRITERS,
                },
            )
            .expect("connect"),
        );
        let times = Arc::new(Mutex::new(vec![]));
        let started = Instant::now();
        let threads: Vec<_> = (0..threads)
            .map(|_| {
                let (pg, times) = (pg.clone(), times.clone());
                thread::spawn(move || {
                    for _ in 0..200 {
                        let t = Instant::now();
                        let mut c = pg.take_conn().expect("take");
                        c.client.query_one("SELECT count(*) FROM pg_class", &[]).expect("query");
                        pg.give_conn(c);
                        times.lock().unwrap().push(t.elapsed());
                    }
                })
            })
            .collect();
        for t in threads {
            t.join().expect("thread");
        }
        let wall = started.elapsed();
        let mut times = times.lock().unwrap().clone();
        times.sort();
        let n = times.len();
        (wall, times[n / 2], times[n * 95 / 100], times[n * 99 / 100], times[n - 1])
    };
    // A sweep, to choose the default from: each limit at a load below it and one well above it.
    for threads in [16, 48] {
        for max_open in [8, 16, memex_store_postgres::DEFAULT_MAX_OPEN, 10_000] {
            let (wall, p50, p95, p99, max) = run(max_open, threads);
            let name = if max_open == 10_000 { "none".to_string() } else { max_open.to_string() };
            println!("{threads} threads x 200 reads, limit {name}: wall {wall:?}, p50 {p50:?} p95 {p95:?} p99 {p99:?} max {max:?}");
        }
    }
}

/// A caller that gives a connection back and takes it again straight away can't starve a waiter: waiters are served in order.
#[test]
#[ignore = "needs Postgres"]
fn pg_pool_waiters_are_served_in_order() {
    let pg = Arc::new(PgStore::connect_with(&named(&unique("pool_fair")), limits(1, 5_000)).expect("connect"));
    let stop = Arc::new(AtomicBool::new(false));
    let hog = {
        let (pg, stop) = (pg.clone(), stop.clone());
        thread::spawn(move || {
            let mut turns = 0;
            while !stop.load(Ordering::Relaxed) {
                let mut c = pg.take_conn().expect("the hog's turn");
                c.client.batch_execute("SELECT pg_sleep(0.002)").expect("query");
                pg.give_conn(c);
                turns += 1;
            }
            turns
        })
    };
    thread::sleep(Duration::from_millis(50));
    let mut waits = vec![];
    for _ in 0..50 {
        let t = Instant::now();
        let c = pg.take_conn().expect("a waiter gets its turn while the hog keeps taking");
        waits.push(t.elapsed());
        pg.give_conn(c);
    }
    stop.store(true, Ordering::Relaxed);
    let turns = hog.join().expect("hog");
    waits.sort();
    println!("50 takes beside a hog ({turns} turns): median wait {:?}, longest {:?}", waits[25], waits[49]);
    assert!(waits[49] < Duration::from_millis(500), "a waiter waited {:?}", waits[49]);
}
