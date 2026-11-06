//! Spec 027 throwaway bench. `decode` and `run` print one JSON line. Only public-domain and synthetic audio goes to the API.
mod decode;
mod gemini;
mod transcriber;
mod retimed;
mod whisper;
use serde_json::json;
use std::path::Path;
use std::time::Instant;
use transcriber::Transcriber;

fn arg(a: &[String], name: &str) -> Option<String> {
    a.iter().position(|x| x == name).and_then(|i| a.get(i + 1)).cloned()
}
fn peak_rss_mb() -> f64 {
    let mut u: libc::rusage = unsafe { std::mem::zeroed() };
    unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut u) };
    u.ru_maxrss as f64 / 1024.0 / 1024.0
}
fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let file = arg(&a, "--file").expect("--file");
    let t = Instant::now();
    let d = match decode::decode(Path::new(&file), !a.iter().any(|x| x == "--no-elst")) {
        Ok(d) => d,
        Err(e) => {
            println!("{}", json!({"file": file, "error": e}));
            std::process::exit(if a[0] == "decode" { 0 } else { 1 });
        }
    };
    let pcm = decode::resample(&d.mono, d.rate, 16_000);
    let decode_s = t.elapsed().as_secs_f64();
    let info = json!({"file": file, "codec": d.codec, "rate": d.rate, "channels": d.channels, "dur_s": pcm.len() as f64 / 16000.0,
        "first_pts_ms": d.first_pts_ms, "elst_shift_ms": d.elst_shift_ms, "gaps": d.gaps, "gap_ms": d.gap_ms, "bad_packets": d.bad_packets, "packets": d.packets, "decode_s": decode_s});
    if a[0] == "decode" {
        if let Some(out) = arg(&a, "--out") {
            let bytes: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
            std::fs::write(out, bytes).unwrap();
        }
        println!("{}", json!({"info": info, "peak_rss_mb": peak_rss_mb()}));
        return;
    }
    // Decoders skip packets they reject. A few is damage; many means the audio is gone, which must not pass as a short transcript.
    if d.bad_packets as u64 * 100 > d.packets {
        eprintln!("{file}: {} of {} packets undecodable, failing the job", d.bad_packets, d.packets);
        std::process::exit(2);
    }
    let lang = arg(&a, "--lang").expect("--lang");
    let chunk: usize = arg(&a, "--chunk").map(|s| s.parse().unwrap()).unwrap_or(0);
    let t = Instant::now();
    let backend = arg(&a, "--backend").expect("--backend");
    let tr: Box<dyn Transcriber> = match backend.as_str() {
        "whisper" => Box::new(
            whisper::WhisperLocal::new(
                &arg(&a, "--model").expect("--model"),
                whisper::Config {
                    threads: arg(&a, "--threads").map(|s| s.parse().unwrap()).unwrap_or(8),
                    gpu: a.iter().any(|x| x == "--gpu"),
                    dtw: arg(&a, "--dtw"),
                    vad: arg(&a, "--vad"),
                    chunk_s: chunk,
                    beam: arg(&a, "--beam").map(|s| s.parse().unwrap()).unwrap_or(1),
                    progress: arg(&a, "--progress"),
                    model: arg(&a, "--model").expect("--model"),
                },
            )
            .expect("model"),
        ),
        "hybrid" => Box::new(retimed::Retimed {
            api: Box::new(gemini::GeminiApi {
                key: std::env::var("GEMINI_API_KEY").expect("GEMINI_API_KEY"),
                model: arg(&a, "--api-model").expect("--api-model"),
                chunk_s: 300,
                max_tokens: arg(&a, "--max-tokens").map(|s| s.parse().unwrap()).unwrap_or(70_000),
                spent: Default::default(),
            }),
            clock: Box::new(
                whisper::WhisperLocal::new(
                    &arg(&a, "--model").expect("--model"),
                    whisper::Config { threads: 8, gpu: true, dtw: arg(&a, "--dtw"), vad: arg(&a, "--vad"), chunk_s: 0, beam: 1, progress: None, model: arg(&a, "--model").unwrap() },
                )
                .expect("model"),
            ),
        }),
        _ => Box::new(gemini::GeminiApi {
            key: std::env::var("GEMINI_API_KEY").expect("GEMINI_API_KEY"),
            model: arg(&a, "--model").expect("--model"),
            chunk_s: if chunk == 0 { 300 } else { chunk },
            max_tokens: arg(&a, "--max-tokens").map(|s| s.parse().unwrap()).unwrap_or(150_000),
            spent: Default::default(),
        }),
    };
    let load_s = t.elapsed().as_secs_f64();
    let t = Instant::now();
    let r = tr.transcribe(&pcm, &lang);
    let wall = t.elapsed().as_secs_f64();
    let r = match r {
        Ok(r) => r,
        Err(e) => {
            println!("{}", json!({"info": info, "error": format!("{e:?}")}));
            std::process::exit(1);
        }
    };
    let words: Vec<_> = r.words.iter().map(|w| json!({"w": w.text, "s": w.start_ms, "e": w.end_ms, "spk": w.speaker})).collect();
    let dur = pcm.len() as f64 / 16000.0;
    let o = json!({"info": info, "load_s": load_s, "wall_s": wall, "x_realtime": dur / wall, "peak_rss_mb": peak_rss_mb(),
        "input_tokens": r.usage.input_tokens, "output_tokens": r.usage.output_tokens, "calls": r.usage.calls, "failed_chunks": r.usage.failed_chunks, "words": words});
    std::fs::write(arg(&a, "--out").expect("--out"), o.to_string()).unwrap();
    println!("{} {} x_realtime={:.1} rss={:.0}MB words={} failed={}", backend, file.rsplit('/').next().unwrap(), dur / wall, peak_rss_mb(), r.words.len(), r.usage.failed_chunks);
}
