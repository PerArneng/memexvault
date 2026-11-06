//! Spec 005 throwaway bench.
//! embed <dir> <file> <threads> <ep> <prefix> <in.txt> <out.f32>: passages in batches of 32, one line each, mean pooled and normalised.
//! latency <dir> <file> <threads> <ep> <prefix> <in.txt>: batch 1 latency per line.
//! rerank <dir> <file> <threads> <ep> <in.jsonl> <out.jsonl>: each line {"q":..,"docs":[..]}, scores out, latency per query.
use ndarray::Array2;
use ort::{session::Session, value::Tensor};
use serde_json::{json, Value};
use std::{io::Write, time::Instant};
use tokenizers::{PaddingParams, Tokenizer, TruncationParams};

fn max_len() -> usize { std::env::var("MAX_LEN").ok().and_then(|v| v.parse().ok()).unwrap_or(256) }

fn session(dir: &str, file: &str, threads: usize, ep: &str) -> Session {
    let mut b = Session::builder().expect("builder").with_intra_threads(threads).expect("threads");
    if ep == "coreml" {
        b = b.with_execution_providers([ort::ep::CoreML::default().build()]).expect("coreml");
    }
    b.commit_from_file(format!("{dir}/{file}")).expect("load")
}

fn tok(dir: &str) -> Tokenizer {
    let mut t = Tokenizer::from_file(format!("{dir}/tokenizer.json")).expect("tokenizer");
    t.with_padding(Some(PaddingParams::default()));
    t.with_truncation(Some(TruncationParams { max_length: max_len(), ..Default::default() })).expect("trunc");
    t
}

/// Runs the encoded batch and returns the first output with its shape.
fn run(s: &mut Session, encs: &[tokenizers::Encoding]) -> (Vec<usize>, Vec<f32>, Array2<i64>) {
    let (n, m) = (encs.len(), encs[0].len());
    let flat = |f: &dyn Fn(&tokenizers::Encoding) -> Vec<u32>| -> Array2<i64> {
        let v: Vec<i64> = encs.iter().flat_map(|e| f(e).into_iter().map(i64::from)).collect();
        Array2::from_shape_vec((n, m), v).expect("shape")
    };
    let ids = flat(&|e| e.get_ids().to_vec());
    let mask = flat(&|e| e.get_attention_mask().to_vec());
    let types = flat(&|e| e.get_type_ids().to_vec());
    let names: Vec<String> = s.inputs().iter().map(|i| i.name().to_string()).collect();
    let mut inputs: Vec<(String, ort::session::SessionInputValue)> = vec![];
    for name in names {
        let a = match name.as_str() { "input_ids" => &ids, "attention_mask" => &mask, "token_type_ids" => &types, o => panic!("input {o}") };
        inputs.push((name, Tensor::from_array(a.clone()).expect("tensor").into()));
    }
    let out = s.run(inputs).expect("run");
    let (shape, data) = out[0].try_extract_tensor::<f32>().expect("f32");
    (shape.iter().map(|&d| d as usize).collect(), data.to_vec(), mask)
}

fn mean_pool(shape: &[usize], data: &[f32], mask: &Array2<i64>) -> Vec<Vec<f32>> {
    let (n, m, d) = (shape[0], shape[1], shape[2]);
    (0..n).map(|i| {
        let mut v = vec![0f32; d];
        let mut c = 0f32;
        for j in 0..m {
            if mask[[i, j]] == 1 {
                for k in 0..d { v[k] += data[(i * m + j) * d + k]; }
                c += 1.0;
            }
        }
        let norm = v.iter().map(|x| (x / c) * (x / c)).sum::<f32>().sqrt();
        v.iter().map(|x| x / c / norm).collect()
    }).collect()
}

fn pct(mut v: Vec<f64>, p: f64) -> f64 {
    v.sort_by(|a, b| a.total_cmp(b));
    v[((v.len() as f64 - 1.0) * p).round() as usize]
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (dir, file, threads, ep) = (&a[2], &a[3], a[4].parse::<usize>().expect("threads"), &a[5]);
    let t0 = Instant::now();
    let mut s = session(dir, file, threads, ep);
    let load = t0.elapsed().as_secs_f64();
    let t = tok(dir);
    match a[1].as_str() {
        "embed" => {
            let lines: Vec<String> = std::fs::read_to_string(&a[7]).expect("in").lines().map(|l| format!("{}{}", a[6], l)).collect();
            let mut out = std::fs::File::create(&a[8]).expect("out");
            let t1 = Instant::now();
            let mut dim = 0;
            for chunk in lines.chunks(32) {
                let encs = t.encode_batch(chunk.iter().map(String::as_str).collect::<Vec<_>>(), true).expect("enc");
                let (sh, d, m) = run(&mut s, &encs);
                dim = sh[2];
                for v in mean_pool(&sh, &d, &m) {
                    out.write_all(&v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>()).expect("w");
                }
            }
            println!("{}", json!({"n": lines.len(), "dim": dim, "per_s": lines.len() as f64 / t1.elapsed().as_secs_f64(), "load_s": load}));
        }
        "latency" => {
            let lines: Vec<String> = std::fs::read_to_string(&a[7]).expect("in").lines().map(|l| format!("{}{}", a[6], l)).collect();
            let mut ms = vec![];
            for l in &lines {
                let t1 = Instant::now();
                let encs = t.encode_batch(vec![l.as_str()], true).expect("enc");
                let (sh, d, m) = run(&mut s, &encs);
                std::hint::black_box(mean_pool(&sh, &d, &m));
                ms.push(t1.elapsed().as_secs_f64() * 1e3);
            }
            println!("{}", json!({"n": ms.len(), "p50_ms": pct(ms.clone(), 0.5), "p95_ms": pct(ms, 0.95), "load_s": load}));
        }
        "rerank" => {
            let mut out = std::fs::File::create(&a[7]).expect("out");
            let mut ms = vec![];
            for line in std::fs::read_to_string(&a[6]).expect("in").lines() {
                let v: Value = serde_json::from_str(line).expect("json");
                let q = v["q"].as_str().expect("q");
                let docs: Vec<&str> = v["docs"].as_array().expect("docs").iter().map(|d| d.as_str().expect("d")).collect();
                let t1 = Instant::now();
                let pairs: Vec<(&str, &str)> = docs.iter().map(|d| (q, *d)).collect();
                let encs = t.encode_batch(pairs, true).expect("enc");
                let (sh, d, _) = run(&mut s, &encs);
                ms.push(t1.elapsed().as_secs_f64() * 1e3);
                let col = sh.get(1).copied().unwrap_or(1);
                let scores: Vec<f32> = d.chunks(col).map(|c| *c.last().expect("c")).collect();
                writeln!(out, "{}", json!({"scores": scores})).expect("w");
            }
            println!("{}", json!({"n": ms.len(), "p50_ms": pct(ms.clone(), 0.5), "p95_ms": pct(ms, 0.95), "load_s": load}));
        }
        c => panic!("{c}"),
    }
}
