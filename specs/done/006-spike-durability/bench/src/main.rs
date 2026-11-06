//! Spec 006 throwaway bench.
//! commit <url> <batch> <event_bytes> <seconds>: closed-loop durable group commit.
//! reads <url> <rows> <n>: single and batched point reads.
//! tantivy <dir> <docs> <n>: watermark payload round trip and JSON-field filters.
//! child <url> <dir>: the crash victim. Commits batches, acks each to stdout, commits tantivy with a watermark.
//! kills <url> <dir> <n> <child|pg> <container>: the crash harness.
use postgres::{Client, NoTls};
use serde_json::json;
use std::{io::{BufRead, BufReader, Write}, process::{Command, Stdio}, sync::{atomic::{AtomicBool, AtomicU64, Ordering}, Arc}, time::{Duration, Instant}};
use tantivy::{collector::{Count, TopDocs}, query::QueryParser, schema::{Schema, Field, TEXT, STRING, STORED, FAST, NumericOptions, JsonObjectOptions, TextFieldIndexing, TextOptions}, doc, Index, IndexWriter, ReloadPolicy, Term};

fn pct(mut v: Vec<f64>, p: f64) -> f64 {
    v.sort_by(|a, b| a.total_cmp(b));
    v[((v.len() as f64 - 1.0) * p).round() as usize]
}

fn setup(c: &mut Client) {
    c.batch_execute("CREATE SCHEMA IF NOT EXISTS d006; DROP TABLE IF EXISTS d006.ev, d006.mat; CREATE TABLE d006.ev (seq bigint PRIMARY KEY, payload bytea NOT NULL); CREATE TABLE d006.mat (seq bigint PRIMARY KEY REFERENCES d006.ev, status int NOT NULL);").expect("ddl");
}

/// One SQL transaction: the event rows and the materialized rows together, as the design commits them.
fn commit_batch(c: &mut Client, seqs: &[i64], payload: &[u8]) -> Result<(), postgres::Error> {
    let bodies: Vec<&[u8]> = seqs.iter().map(|_| payload).collect();
    let statuses: Vec<i32> = seqs.iter().map(|_| 1).collect();
    let mut t = c.transaction()?;
    t.execute("INSERT INTO d006.ev SELECT * FROM unnest($1::bigint[], $2::bytea[])", &[&seqs, &bodies])?;
    t.execute("INSERT INTO d006.mat SELECT * FROM unnest($1::bigint[], $2::int[])", &[&seqs, &statuses])?;
    t.commit()
}

fn commit(url: &str, batch: usize, bytes: usize, secs: f64) {
    let mut c = Client::connect(url, NoTls).expect("connect");
    setup(&mut c);
    let settings: Vec<String> = ["fsync", "synchronous_commit", "wal_sync_method", "full_page_writes"].iter().map(|s| { let r = c.query_one(&format!("SHOW {s}"), &[]).expect("show"); format!("{s}={}", r.get::<_, String>(0)) }).collect();
    let payload = vec![7u8; bytes];
    let (mut ms, mut seq, t0) = (vec![], 0i64, Instant::now());
    while t0.elapsed().as_secs_f64() < secs {
        let seqs: Vec<i64> = (seq..seq + batch as i64).collect();
        let t = Instant::now();
        commit_batch(&mut c, &seqs, &payload).expect("commit");
        ms.push(t.elapsed().as_secs_f64() * 1e3);
        seq += batch as i64;
    }
    println!("{}", json!({"batch": batch, "bytes": bytes, "events_per_s": seq as f64 / t0.elapsed().as_secs_f64(), "commit_p50": pct(ms.clone(), 0.5), "commit_p95": pct(ms.clone(), 0.95), "commit_p99": pct(ms, 0.99), "settings": settings}));
}

fn reads(url: &str, rows: i64, n: usize) {
    let mut c = Client::connect(url, NoTls).expect("connect");
    let name = format!("d006.obj{rows}");
    c.batch_execute(&format!("CREATE SCHEMA IF NOT EXISTS d006; DROP TABLE IF EXISTS {name}; CREATE TABLE {name} (id bigint PRIMARY KEY, version int NOT NULL, body text NOT NULL); INSERT INTO {name} SELECT g, 1, repeat('x', 1000) FROM generate_series(0, {} ) g; ANALYZE {name};", rows - 1)).expect("load");
    let one = c.prepare(&format!("SELECT id, version, body FROM {name} WHERE id = $1")).expect("p");
    let many = c.prepare(&format!("SELECT id, version, body FROM {name} WHERE id = ANY($1)")).expect("p");
    let (mut a, mut b) = (vec![], vec![]);
    for _ in 0..n {
        let id = fastrand::i64(0..rows);
        let t = Instant::now();
        assert_eq!(c.query(&one, &[&id]).expect("q").len(), 1);
        a.push(t.elapsed().as_secs_f64() * 1e3);
        let ids: Vec<i64> = (0..50).map(|_| fastrand::i64(0..rows)).collect();
        let t = Instant::now();
        c.query(&many, &[&ids]).expect("q");
        b.push(t.elapsed().as_secs_f64() * 1e3);
    }
    println!("{}", json!({"rows": rows, "single_p50": pct(a.clone(), 0.5), "single_p95": pct(a, 0.95), "batch50_p50": pct(b.clone(), 0.5), "batch50_p95": pct(b, 0.95)}));
}

struct Fields { seq: Field, body: Field, attrs: Field }

fn schema() -> (Schema, Fields) {
    let mut b = Schema::builder();
    let seq = b.add_u64_field("seq", NumericOptions::default().set_indexed().set_fast());
    let body = b.add_text_field("body", TEXT);
    let opts = JsonObjectOptions::default().set_fast("raw").set_indexing_options(tantivy::schema::TextFieldIndexing::default().set_tokenizer("raw").set_index_option(tantivy::schema::IndexRecordOption::Basic));
    let attrs = b.add_json_field("attrs", opts);
    let _ = (STRING, STORED, FAST, TextOptions::default(), TextFieldIndexing::default());
    (b.build(), Fields { seq, body, attrs })
}

fn open(dir: &str) -> (Index, Fields) {
    std::fs::create_dir_all(dir).expect("dir");
    let (s, f) = schema();
    let d = tantivy::directory::MmapDirectory::open(dir).expect("mmap");
    (Index::open_or_create(d, s).expect("open"), f)
}

fn watermark(i: &Index) -> i64 {
    i.load_metas().expect("metas").payload.and_then(|p| p.parse().ok()).unwrap_or(-1)
}

fn tantivy_filters(dir: &str, docs: u64, n: usize) {
    let _ = std::fs::remove_dir_all(dir);
    let (index, f) = open(dir);
    let t = Instant::now();
    let mut w: IndexWriter = index.writer(500_000_000).expect("w");
    let colors = ["red", "green", "blue", "amber", "violet"];
    for i in 0..docs {
        let d = match i % 3 {
            0 => json!({"product": {"color": colors[(i % 5) as usize], "weight": (i % 1000) as f64 / 10.0, "sku": format!("SKU-{i}")}}),
            1 => json!({"person": {"city": format!("city{}", i % 200), "born": 1950 + (i % 60), "k": format!("key{}", i % 50)}}),
            _ => json!({"project": {"state": if i % 2 == 0 { "open" } else { "closed" }, "budget": i % 100_000, "custom_field_x": format!("v{}", i % 1000)}}),
        };
        w.add_document(doc!(f.seq => i, f.body => format!("segment text number {i} about city{}", i % 200), f.attrs => d)).expect("add");
    }
    let mut pc = w.prepare_commit().expect("prepare");
    pc.set_payload(&docs.to_string());
    pc.commit().expect("commit");
    let build = t.elapsed().as_secs_f64();
    drop(w);
    // Watermark round trip through a fresh open.
    let (reopened, _) = open(dir);
    let wm = watermark(&reopened);
    let r = reopened.reader_builder().reload_policy(ReloadPolicy::Manual).try_into().expect("reader");
    let s = r.searcher();
    let qp = QueryParser::for_index(&reopened, vec![f.body, f.attrs]);
    let cases = [("term", "attrs.product.color:amber"), ("term_rare", "attrs.project.custom_field_x:v77"), ("range", "attrs.product.weight:[10 TO 12]"), ("range_int", "attrs.person.born:[1960 TO 1961]"), ("combo", "attrs.product.color:amber AND attrs.product.weight:[10 TO 50] AND body:segment")];
    let mut out = vec![];
    for (name, q) in cases {
        let q = qp.parse_query(q).expect("parse");
        let (mut ms, mut hits) = (vec![], 0);
        for _ in 0..n {
            let t = Instant::now();
            let (c, top) = s.search(&q, &(Count, TopDocs::with_limit(50).order_by_score())).expect("search");
            ms.push(t.elapsed().as_secs_f64() * 1e3);
            hits = c;
            std::hint::black_box(top);
        }
        out.push(json!({"q": name, "hits": hits, "p50": pct(ms.clone(), 0.5), "p95": pct(ms, 0.95)}));
    }
    let size: u64 = std::fs::read_dir(dir).expect("d").map(|e| e.expect("e").metadata().expect("m").len()).sum();
    println!("{}", json!({"docs": docs, "watermark": wm, "build_s": build, "disk_mb": size / 1_000_000, "cases": out}));
}

fn child(url: &str, dir: &str) {
    let mut c = Client::connect(url, NoTls).expect("connect");
    let (index, f) = open(dir);
    let mut w: IndexWriter = index.writer_with_num_threads(1, 50_000_000).expect("w");
    let top: i64 = c.query_one("SELECT coalesce(max(seq), -1) FROM d006.ev", &[]).expect("max").get(0);
    let term = |s: i64| Term::from_field_u64(f.seq, s as u64);
    // Resume: replay anything above the watermark. Idempotent by key.
    let wm = watermark(&index);
    let rows = c.query("SELECT seq FROM d006.ev WHERE seq > $1 ORDER BY seq", &[&wm]).expect("replay");
    for r in &rows {
        let s: i64 = r.get(0);
        w.delete_term(term(s));
        w.add_document(doc!(f.seq => s as u64, f.body => format!("event {s}"))).expect("add");
    }
    if !rows.is_empty() {
        let mut pc = w.prepare_commit().expect("prepare");
        pc.set_payload(&top.to_string());
        pc.commit().expect("commit");
    }
    println!("ready {top}");
    std::io::stdout().flush().expect("flush");
    let payload = vec![7u8; 512];
    let mut seq = top + 1;
    let mut since = 0;
    loop {
        let batch = 1 + fastrand::usize(0..20);
        let seqs: Vec<i64> = (seq..seq + batch as i64).collect();
        if commit_batch(&mut c, &seqs, &payload).is_err() { std::process::exit(3); }
        seq += batch as i64;
        println!("ack {}", seq - 1);
        std::io::stdout().flush().expect("flush");
        for s in &seqs {
            w.delete_term(term(*s));
            w.add_document(doc!(f.seq => *s as u64, f.body => format!("event {s}"))).expect("add");
        }
        since += 1;
        if since >= 5 {
            let mut pc = w.prepare_commit().expect("prepare");
            pc.set_payload(&(seq - 1).to_string());
            pc.commit().expect("commit");
            since = 0;
        }
    }
}

fn wait_pg(url: &str) -> Client {
    let t = Instant::now();
    loop {
        if let Ok(c) = Client::connect(url, NoTls) { return c; }
        if t.elapsed().as_secs() > 120 { panic!("postgres did not return"); }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn kills(url: &str, dir: &str, n: usize, mode: &str, container: &str) {
    let me = std::env::current_exe().expect("exe");
    let (mut lost, mut partial, mut gaps, mut ahead, mut reopen_fail, mut dup_or_missing) = (0, 0, 0, 0, 0, 0);
    let (mut total_acked, mut replayed) = (0i64, 0i64);
    let mut recover = vec![];
    let t0 = Instant::now();
    for run in 0..n {
        let mut ch = Command::new(&me).args(["child", url, dir]).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().expect("spawn");
        let out = ch.stdout.take().expect("out");
        let last = Arc::new(AtomicU64::new(0));
        let got = Arc::new(AtomicBool::new(false));
        let (l2, g2) = (last.clone(), got.clone());
        let th = std::thread::spawn(move || {
            for line in BufReader::new(out).lines().map_while(Result::ok) {
                if let Some(v) = line.strip_prefix("ack ") { l2.store(v.parse::<u64>().expect("n") + 1, Ordering::SeqCst); g2.store(true, Ordering::SeqCst); }
                if let Some(v) = line.strip_prefix("ready ") { let x: i64 = v.parse().expect("n"); l2.store((x + 1) as u64, Ordering::SeqCst); }
            }
        });
        // Wait until it's writing, then kill at a random moment.
        while !got.load(Ordering::SeqCst) { std::thread::sleep(Duration::from_millis(1)); }
        std::thread::sleep(Duration::from_millis(fastrand::u64(1..300)));
        if mode == "pg" {
            Command::new("docker").args(["kill", "-s", "KILL", container]).output().expect("docker kill");
            let _ = ch.wait();
        } else {
            ch.kill().expect("kill");
            ch.wait().expect("wait");
        }
        th.join().expect("join");
        let acked = last.load(Ordering::SeqCst) as i64 - 1; // highest acknowledged seq
        let t = Instant::now();
        if mode == "pg" { Command::new("docker").args(["start", container]).output().expect("docker start"); }
        let mut c = wait_pg(url);
        if mode == "pg" { recover.push(t.elapsed().as_secs_f64()); }
        let r = c.query_one("SELECT coalesce(max(seq), -1), count(*) FROM d006.ev", &[]).expect("q");
        let (max, count): (i64, i64) = (r.get(0), r.get(1));
        let m: i64 = c.query_one("SELECT count(*) FROM d006.mat", &[]).expect("q").get(0);
        if max < acked { lost += 1; eprintln!("run {run}: lost {acked} > {max}"); }
        if count != max + 1 { gaps += 1; }
        if m != count { partial += 1; }
        total_acked = acked;
        // The index must reopen and never be ahead of the log.
        let opened = std::panic::catch_unwind(|| {
            let (i, _) = open(dir);
            let wm = watermark(&i);
            let rd = i.reader().expect("reader");
            (wm, rd.searcher().num_docs())
        });
        match opened {
            Err(_) => reopen_fail += 1,
            Ok((wm, docs)) => {
                if wm > max { ahead += 1; eprintln!("run {run}: watermark {wm} > {max}"); }
                if docs != (wm + 1) as u64 { dup_or_missing += 1; eprintln!("run {run}: docs {docs} wm {wm}"); }
                replayed += max - wm;
            }
        }
        if run % 50 == 0 { eprintln!("run {run} acked {acked} {:.0}s", t0.elapsed().as_secs_f64()); }
    }
    println!("{}", json!({"mode": mode, "runs": n, "highest_acked": total_acked, "acked_lost": lost, "partial_tx": partial, "gaps": gaps, "watermark_ahead": ahead, "reopen_failures": reopen_fail, "index_doc_mismatch": dup_or_missing, "events_above_watermark_total": replayed, "minutes": t0.elapsed().as_secs_f64() / 60.0, "pg_recover_p95_s": if recover.is_empty() { 0.0 } else { pct(recover, 0.95) }}));
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    match a[1].as_str() {
        "commit" => commit(&a[2], a[3].parse().expect("b"), a[4].parse().expect("s"), a[5].parse().expect("t")),
        "reads" => reads(&a[2], a[3].parse().expect("r"), a[4].parse().expect("n")),
        "tantivy" => tantivy_filters(&a[2], a[3].parse().expect("d"), a[4].parse().expect("n")),
        "setup" => setup(&mut Client::connect(&a[2], NoTls).expect("connect")),
        "child" => child(&a[2], &a[3]),
        "kills" => kills(&a[2], &a[3], a[4].parse().expect("n"), &a[5], &a[6]),
        c => panic!("{c}"),
    }
}
