//! Spec 004 throwaway bench.
//! load <url> <table> <p.f32> <dim> <rows> <vector|halfvec>: COPY rows, reports seconds.
//! index <url> <table> <dim-type> <m> <ef_c>: builds HNSW, reports seconds and sizes.
//! query <url> <table> <dim-type> <q.f32> <dim> <gt.u32> <pct> <n> <threads> <ef> <iter> <max_scan> [writer_per_s] : recall@50 and latency.
//! flat <p.f32> <q.f32> <dim> <gt.u32> <rows> <pct> <del> <n> <threads> <rescore>: binary scan then float rescore, all in memory.
use pgvector::Vector;
use postgres::{Client, NoTls};
use serde_json::json;
use std::{collections::HashSet, sync::{atomic::{AtomicBool, Ordering}, Arc}, time::{Duration, Instant}};

fn floats(path: &str) -> Vec<f32> {
    std::fs::read(path).expect("read").chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect()
}
fn hash(id: u64) -> u64 { id.wrapping_mul(2654435761) % 4294967296 }
fn bucket(id: u64) -> i32 { ((hash(id) >> 8) % 100) as i32 }
fn deleted(id: u64) -> bool { (hash(id) >> 20) % 5 == 0 }
fn pct(mut v: Vec<f64>, p: f64) -> f64 {
    v.sort_by(|a, b| a.total_cmp(b));
    v[((v.len() as f64 - 1.0) * p).round() as usize]
}
fn truth(path: &str) -> Vec<Vec<u32>> {
    std::fs::read(path).expect("gt").chunks_exact(4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect::<Vec<_>>().chunks(50).map(<[u32]>::to_vec).collect()
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    match a[1].as_str() {
        "load" => load(&a[2], &a[3], &a[4], a[5].parse().expect("dim"), a[6].parse().expect("rows"), &a[7]),
        "index" => index(&a[2], &a[3], &a[4], &a[5], &a[6]),
        "query" => query(&a),
        "flat" => flat(&a),
        "usearch" => usearch_run(&a),
        c => panic!("{c}"),
    }
}

fn load(url: &str, table: &str, file: &str, dim: usize, rows: usize, kind: &str) {
    let v = floats(file);
    let mut c = Client::connect(url, NoTls).expect("connect");
    c.batch_execute(&format!("CREATE EXTENSION IF NOT EXISTS vector; DROP TABLE IF EXISTS {table}; CREATE TABLE {table} (id bigint PRIMARY KEY, bucket int NOT NULL, embedding {kind}({dim}) NOT NULL);")).expect("ddl");
    let t = Instant::now();
    let mut w = c.copy_in(&format!("COPY {table} FROM STDIN")).expect("copy");
    use std::io::Write;
    for (i, row) in v.chunks(dim).take(rows).enumerate() {
        let body: Vec<String> = row.iter().map(|x| format!("{x}")).collect();
        writeln!(w, "{i}\t{}\t[{}]", bucket(i as u64), body.join(",")).expect("w");
    }
    w.finish().expect("finish");
    c.batch_execute(&format!("CREATE INDEX ON {table} (bucket); ANALYZE {table};")).expect("idx");
    println!("{}", json!({"table": table, "rows": rows, "load_s": t.elapsed().as_secs_f64()}));
}

fn index(url: &str, table: &str, kind: &str, m: &str, ef_c: &str) {
    let mut c = Client::connect(url, NoTls).expect("connect");
    c.batch_execute("SET maintenance_work_mem='1500MB'; SET max_parallel_maintenance_workers=7;").expect("set");
    c.batch_execute(&format!("DROP INDEX IF EXISTS {table}_hnsw")).expect("drop");
    let t = Instant::now();
    let ip = std::env::var("IP").is_ok();
    let ops = match (kind, ip) { ("halfvec", true) => "halfvec_ip_ops", ("halfvec", false) => "halfvec_cosine_ops", (_, true) => "vector_ip_ops", _ => "vector_cosine_ops" };
    c.batch_execute(&format!("CREATE INDEX {table}_hnsw ON {table} USING hnsw (embedding {ops}) WITH (m={m}, ef_construction={ef_c})")).expect("index");
    let s = t.elapsed().as_secs_f64();
    let r = c.query_one(&format!("SELECT pg_relation_size('{table}_hnsw')::bigint, pg_total_relation_size('{table}')::bigint"), &[]).expect("size");
    let (i, tot): (i64, i64) = (r.get(0), r.get(1));
    println!("{}", json!({"table": table, "m": m, "ef_c": ef_c, "build_s": s, "index_mb": i / 1_000_000, "total_mb": tot / 1_000_000}));
}

fn query(a: &[String]) {
    let (url, table, kind, qf, dim, gtf) = (a[2].clone(), a[3].clone(), a[4].clone(), &a[5], a[6].parse::<usize>().expect("dim"), &a[7]);
    let (p, n, threads, ef, iter, max_scan): (i32, usize, usize, String, String, String) = (a[8].parse().expect("pct"), a[9].parse().expect("n"), a[10].parse().expect("t"), a[11].clone(), a[12].clone(), a[13].clone());
    let writer: usize = a.get(14).map_or(0, |v| v.parse().expect("writer"));
    let q = Arc::new(floats(qf));
    let gt = Arc::new(truth(gtf));
    let n = n.min(gt.len());
    let stop = Arc::new(AtomicBool::new(false));
    // A writer inserting at a fixed rate, in the id range above the corpus, reusing query vectors.
    let wr = (writer > 0).then(|| {
        let (url, table, kind, q, stop) = (url.clone(), table.clone(), kind.clone(), q.clone(), stop.clone());
        std::thread::spawn(move || {
            let mut c = Client::connect(&url, NoTls).expect("connect");
            c.batch_execute(&format!("DELETE FROM {table} WHERE id >= 100000000")).expect("clean");
            let mut id = 100_000_000i64;
            let start = Instant::now();
            let mut done = 0usize;
            while !stop.load(Ordering::Relaxed) {
                let due = (start.elapsed().as_secs_f64() * writer as f64) as usize;
                if done >= due { std::thread::sleep(Duration::from_millis(2)); continue; }
                let row = q[(id as usize % (q.len() / dim)) * dim..][..dim].to_vec();
                let cast = if kind == "halfvec" { format!("$2::vector::halfvec({dim})") } else { "$2".to_string() };
                let sql = format!("INSERT INTO {table} VALUES ($1, 100, {cast})");
                c.execute(&sql, &[&id, &Vector::from(row)]).expect("ins");
                id += 1; done += 1;
            }
            let d = done as f64 / start.elapsed().as_secs_f64();
            c.batch_execute(&format!("DELETE FROM {table} WHERE id >= 100000000")).expect("clean");
            d
        })
    });
    let t0 = Instant::now();
    let per = n / threads;
    let hs: Vec<_> = (0..threads).map(|th| {
        let (url, table, kind, q, gt, ef, iter, max_scan) = (url.clone(), table.clone(), kind.clone(), q.clone(), gt.clone(), ef.clone(), iter.clone(), max_scan.clone());
        std::thread::spawn(move || {
            let mut c = Client::connect(&url, NoTls).expect("connect");
            c.batch_execute(&format!("SET hnsw.ef_search={ef}; SET hnsw.iterative_scan={iter}; SET hnsw.max_scan_tuples={max_scan}; SET jit=off; {}", std::env::var("EXTRA_SET").unwrap_or_default())).expect("set");
            let filter = if p >= 100 { String::new() } else { format!("WHERE bucket < {p}") };
            let target = if kind == "halfvec" { format!("$1::vector::halfvec({dim})") } else { "$1".to_string() };
            let op = if std::env::var("IP").is_ok() { "<#>" } else { "<=>" };
            let sql = match std::env::var("CTE_LIMIT") {
                Ok(lim) => format!("WITH c AS MATERIALIZED (SELECT id, embedding {op} {target} AS d FROM {table} {filter} ORDER BY d LIMIT {lim}) SELECT id FROM c ORDER BY d LIMIT 50"),
                Err(_) => format!("SELECT id FROM {table} {filter} ORDER BY embedding {op} {target} LIMIT 50"),
            };
            let st = c.prepare(&sql).expect("prepare");
            let (mut ms, mut rec) = (vec![], vec![]);
            for i in (th * per)..((th + 1) * per) {
                let row = q[i * dim..][..dim].to_vec();
                let t = Instant::now();
                let rows = c.query(&st, &[&Vector::from(row)]).expect("q");
                ms.push(t.elapsed().as_secs_f64() * 1e3);
                let got: HashSet<u32> = rows.iter().map(|r| r.get::<_, i64>(0) as u32).collect();
                rec.push(gt[i].iter().filter(|x| got.contains(x)).count() as f64 / 50.0);
            }
            (ms, rec)
        })
    }).collect();
    let (mut ms, mut rec) = (vec![], vec![]);
    for h in hs { let (m, r) = h.join().expect("join"); ms.extend(m); rec.extend(r); }
    let wall = t0.elapsed().as_secs_f64();
    stop.store(true, Ordering::Relaxed);
    let w = wr.map(|h| h.join().expect("w"));
    let recall = rec.iter().sum::<f64>() / rec.len() as f64;
    println!("{}", json!({"pct": p, "threads": threads, "ef": ef, "iter": iter, "recall": (recall * 1000.0).round() / 1000.0, "p50": pct(ms.clone(), 0.5), "p95": pct(ms.clone(), 0.95), "p99": pct(ms, 0.99), "qps": n as f64 / wall, "writer_per_s": w}));
}

fn flat(a: &[String]) {
    let dim: usize = a[4].parse().expect("dim");
    let rows: usize = a[6].parse().expect("rows");
    let (p, del, n, threads, rescore): (i32, bool, usize, usize, usize) = (a[7].parse().expect("pct"), a[8] == "del", a[9].parse().expect("n"), a[10].parse().expect("t"), a[11].parse().expect("rescore"));
    let v = floats(&a[2]);
    let q = floats(&a[3]);
    let gt = truth(&a[5]);
    let n = n.min(gt.len());
    let words = dim.div_ceil(64);
    let code = |row: &[f32]| -> Vec<u64> {
        let mut c = vec![0u64; words];
        for (i, x) in row.iter().enumerate() { if *x > 0.0 { c[i / 64] |= 1 << (i % 64); } }
        c
    };
    let t = Instant::now();
    let codes: Vec<u64> = v.chunks(dim).take(rows).flat_map(code).collect();
    let keep: Vec<u32> = (0..rows as u64).filter(|i| bucket(*i) < p && !(del && deleted(*i))).map(|i| i as u32).collect();
    let build = t.elapsed().as_secs_f64();
    // Int8 rescoring: the stored form the design would use.
    let i8s: Vec<i8> = v.iter().take(rows * dim).map(|x| (x * 127.0 * 3.0).clamp(-127.0, 127.0) as i8).collect();
    let (codes, keep, i8s, q, gt) = (Arc::new(codes), Arc::new(keep), Arc::new(i8s), Arc::new(q), Arc::new(gt));
    let per = n / threads;
    let hs: Vec<_> = (0..threads).map(|th| {
        let (codes, keep, i8s, q, gt) = (codes.clone(), keep.clone(), i8s.clone(), q.clone(), gt.clone());
        std::thread::spawn(move || {
            let (mut ms, mut rec) = (vec![], vec![]);
            for i in (th * per)..((th + 1) * per) {
                let row = &q[i * dim..][..dim];
                let t = Instant::now();
                let mut qc = vec![0u64; words];
                for (j, x) in row.iter().enumerate() { if *x > 0.0 { qc[j / 64] |= 1 << (j % 64); } }
                let mut cand: Vec<(u32, u32)> = keep.iter().map(|&id| {
                    let c = &codes[id as usize * words..][..words];
                    (c.iter().zip(&qc).map(|(a, b)| (a ^ b).count_ones()).sum::<u32>(), id)
                }).collect();
                let r = rescore.min(cand.len());
                if r < cand.len() { cand.select_nth_unstable(r); cand.truncate(r); }
                let mut scored: Vec<(f32, u32)> = cand.iter().map(|&(_, id)| {
                    let s: f32 = i8s[id as usize * dim..][..dim].iter().zip(row).map(|(a, b)| f32::from(*a) * b).sum();
                    (-s, id)
                }).collect();
                let k = 50.min(scored.len());
                if k < scored.len() { scored.select_nth_unstable_by(k, |a, b| a.0.total_cmp(&b.0)); }
                ms.push(t.elapsed().as_secs_f64() * 1e3);
                let got: HashSet<u32> = scored[..k].iter().map(|x| x.1).collect();
                rec.push(gt[i].iter().filter(|x| got.contains(x)).count() as f64 / 50.0);
            }
            (ms, rec)
        })
    }).collect();
    let (mut ms, mut rec) = (vec![], vec![]);
    for h in hs { let (m, r) = h.join().expect("join"); ms.extend(m); rec.extend(r); }
    let recall = rec.iter().sum::<f64>() / rec.len() as f64;
    println!("{}", json!({"flat": rows, "pct": p, "del": del, "rescore": rescore, "threads": threads, "recall": (recall * 1000.0).round() / 1000.0, "p50": pct(ms.clone(), 0.5), "p95": pct(ms, 0.95), "build_s": build, "bits_mb": rows * words * 8 / 1_000_000, "i8_mb": rows * dim / 1_000_000}));
}

/// usearch <p.f32> <q.f32> <dim> <rows> <m> <ef_c> <dir> <i8|f32> then cases "gtfile pct del ef" run on 8 threads and 1.
fn usearch_run(a: &[String]) {
    use usearch::{Index, IndexOptions, MetricKind, ScalarKind};
    let (dim, rows, m, ef_c): (usize, usize, usize, usize) = (a[4].parse().expect("d"), a[5].parse().expect("r"), a[6].parse().expect("m"), a[7].parse().expect("e"));
    let (path, quant) = (format!("{}/usearch_{rows}_{}.idx", a[8], a[9]), if a[9] == "i8" { ScalarKind::I8 } else { ScalarKind::F32 });
    let opts = IndexOptions { dimensions: dim, metric: MetricKind::IP, quantization: quant, connectivity: m, expansion_add: ef_c, expansion_search: 100, multi: false };
    let v = floats(&a[2]);
    let q = Arc::new(floats(&a[3]));
    let index = Index::new(&opts).expect("new");
    index.reserve(rows).expect("reserve");
    let t = Instant::now();
    let index = Arc::new(index);
    let chunks: Vec<Vec<(u64, Vec<f32>)>> = { let mut c = vec![vec![]; 8]; for (i, r) in v.chunks(dim).take(rows).enumerate() { c[i % 8].push((i as u64, r.to_vec())); } c };
    let hs: Vec<_> = chunks.into_iter().map(|chunk| { let index = index.clone(); std::thread::spawn(move || { for (k, r) in chunk { index.add(k, &r).expect("add"); } }) }).collect();
    for h in hs { h.join().expect("j"); }
    let build = t.elapsed().as_secs_f64();
    index.save(&path).expect("save");
    drop(v);
    let t = Instant::now();
    let view = Index::new(&opts).expect("new");
    view.view(&path).expect("view");
    let open_ms = t.elapsed().as_secs_f64() * 1e3;
    println!("{}", json!({"usearch_rows": rows, "quant": a[9], "build_s": build, "file_mb": std::fs::metadata(&path).expect("m").len() / 1_000_000, "open_view_ms": open_ms}));
    let view = Arc::new(view);
    for case in &a[10..] {
        let parts: Vec<&str> = case.split(':').collect();
        let (gtf, p, del, ef): (String, i32, bool, usize) = (parts[0].to_string(), parts[1].parse().expect("p"), parts[2] == "del", parts[3].parse().expect("ef"));
        let threads: usize = parts[4].parse().expect("threads");
        view.change_expansion_search(ef);
        let gt = Arc::new(truth(&gtf));
        let per = gt.len() / threads;
        let hs: Vec<_> = (0..threads).map(|th| { let (view, q, gt) = (view.clone(), q.clone(), gt.clone()); std::thread::spawn(move || {
            let (mut ms, mut rec) = (vec![], vec![]);
            for i in (th * per)..((th + 1) * per) {
                let row = &q[i * dim..][..dim];
                let t = Instant::now();
                let m = if p >= 100 && !del { view.search(row, 50).expect("s") } else { view.filtered_search(row, 50, |k: u64| bucket(k) < p && !(del && deleted(k))).expect("fs") };
                ms.push(t.elapsed().as_secs_f64() * 1e3);
                let got: HashSet<u32> = m.keys.iter().map(|k| *k as u32).collect();
                rec.push(gt[i].iter().filter(|x| got.contains(x)).count() as f64 / 50.0);
            }
            (ms, rec) }) }).collect();
        let (mut ms, mut rec) = (vec![], vec![]);
        for h in hs { let (m, r) = h.join().expect("j"); ms.extend(m); rec.extend(r); }
        println!("usearch {} {:>3}% {} t{threads} ef{ef} recall {:.3} p50 {:.2} p95 {:.2} p99 {:.2}", a[9], p, parts[2], rec.iter().sum::<f64>() / rec.len() as f64, pct(ms.clone(), 0.5), pct(ms.clone(), 0.95), pct(ms, 0.99));
    }
}
