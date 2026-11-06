//! Local implementation on whisper-rs with bundled whisper.cpp. Word times come from token times.
use crate::transcriber::*;
use whisper_rs::{DtwMode, DtwModelPreset, DtwParameters, FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState, WhisperVadContext, WhisperVadContextParams, WhisperVadParams};

pub struct Config {
    pub threads: i32,
    pub gpu: bool,
    pub dtw: Option<String>,
    pub vad: Option<String>,
    pub chunk_s: usize,
    pub beam: i32,
    /// Spans done so far, so a new process can carry on after whisper.cpp aborts. See `transcribe`.
    pub progress: Option<String>,
    pub model: String,
}
pub struct WhisperLocal {
    ctx: WhisperContext,
    cfg: Config,
}
impl WhisperLocal {
    pub fn new(model: &str, cfg: Config) -> Result<Self, TranscribeError> {
        let mut p = WhisperContextParameters::default();
        p.use_gpu(cfg.gpu);
        if let Some(d) = &cfg.dtw {
            let preset = match d.as_str() {
                "base" => DtwModelPreset::Base,
                "small" => DtwModelPreset::Small,
                _ => DtwModelPreset::LargeV3Turbo,
            };
            p.dtw_parameters(DtwParameters { mode: DtwMode::ModelPreset { model_preset: preset }, dtw_mem_size: 1024 * 1024 * 256 });
        }
        let ctx = WhisperContext::new_with_params(model, p).map_err(|e| TranscribeError::Unavailable(e.to_string()))?;
        Ok(Self { ctx, cfg })
    }
    /// Speech spans in samples. `state.full` calls `whisper_full_with_state`, which ignores `params.vad`, so VAD runs here.
    /// Spans closer than 1 s are merged, up to 120 s each, so whisper still gets context across short pauses.
    fn speech_spans(&self, model: &str, pcm: &[f32]) -> Result<Vec<(usize, usize)>, TranscribeError> {
        let mut cp = WhisperVadContextParams::new();
        cp.set_n_threads(self.cfg.threads);
        cp.set_use_gpu(false);
        let mut vad = WhisperVadContext::new(model, cp).map_err(|e| TranscribeError::Unavailable(e.to_string()))?;
        let mut vp = WhisperVadParams::new();
        vp.set_speech_pad(200);
        let segs = vad.segments_from_samples(vp, pcm).map_err(|e| TranscribeError::Provider(e.to_string()))?;
        let mut spans: Vec<(usize, usize)> = Vec::new();
        // Segment times are in centiseconds.
        for s in segs {
            let (a, b) = ((s.start * 160.0) as usize, ((s.end * 160.0) as usize).min(pcm.len()));
            match spans.last_mut() {
                Some(l) if a <= l.1 + 16_000 && b - l.0 <= 120 * 16_000 => l.1 = b,
                _ => spans.push((a, b)),
            }
        }
        Ok(spans)
    }
    fn run(&self, state: &mut WhisperState, ctx: &WhisperContext, use_dtw: bool, pcm: &[f32], language: &str, base_ms: i64, out: &mut Vec<Word>) -> Result<(), TranscribeError> {
        let strategy = if self.cfg.beam > 1 {
            SamplingStrategy::BeamSearch { beam_size: self.cfg.beam, patience: -1.0 }
        } else {
            SamplingStrategy::Greedy { best_of: 1 }
        };
        let mut f = FullParams::new(strategy);
        f.set_language(Some(language));
        f.set_n_threads(self.cfg.threads);
        f.set_token_timestamps(true);
        f.set_print_progress(false);
        f.set_print_realtime(false);
        f.set_print_special(false);
        f.set_print_timestamps(false);
        // Without this, one bad window can repeat itself through the rest of a long file.
        f.set_no_context(true);
        state.full(f, pcm).map_err(|e| TranscribeError::Provider(e.to_string()))?;
        let eot = ctx.token_eot();
        for seg in state.as_iter() {
            let mut cur: Option<(Vec<u8>, i64, i64)> = None;
            for i in 0..seg.n_tokens() {
                let Some(tok) = seg.get_token(i) else { continue };
                let d = tok.token_data();
                if d.id >= eot {
                    continue;
                }
                let bytes = tok.to_bytes().map_err(|e| TranscribeError::Provider(e.to_string()))?.to_vec();
                let starts_word = bytes.first() == Some(&b' ');
                let t0 = if use_dtw && d.t_dtw >= 0 { d.t_dtw } else { d.t0 } * 10;
                let t1 = d.t1 * 10;
                match (&mut cur, starts_word) {
                    (Some(c), false) => {
                        c.0.extend_from_slice(&bytes);
                        c.2 = t1;
                    }
                    _ => {
                        flush(&mut cur, base_ms, out);
                        cur = Some((bytes, t0, t1));
                    }
                }
            }
            flush(&mut cur, base_ms, out);
        }
        if self.cfg.dtw.is_some() {
            // DTW gives starts only, so a word ends at the next word's start, capped so a pause isn't swallowed.
            for i in 0..out.len().saturating_sub(1) {
                let next = out[i + 1].start_ms;
                if next > out[i].start_ms && next - out[i].start_ms < 1500 {
                    out[i].end_ms = next;
                }
            }
        }
        Ok(())
    }
}
fn flush(cur: &mut Option<(Vec<u8>, i64, i64)>, base: i64, out: &mut Vec<Word>) {
    if let Some((b, s, e)) = cur.take() {
        let text = String::from_utf8_lossy(&b).trim().to_string();
        if !text.is_empty() {
            out.push(Word { text, start_ms: s + base, end_ms: e + base, speaker: None });
        }
    }
}
impl Transcriber for WhisperLocal {
    fn transcribe(&self, pcm: &[f32], language: &str) -> Result<Transcript, TranscribeError> {
        let mut words = Vec::new();
        let mut usage = Usage::default();
        let parts = match &self.cfg.vad {
            Some(m) => self.speech_spans(m, pcm)?,
            None if self.cfg.chunk_s == 0 => vec![(0, pcm.len())],
            None => chunks(pcm, self.cfg.chunk_s),
        };
        eprintln!("whisper parts={} audio_s={}", parts.len(), parts.iter().map(|(a, b)| b - a).sum::<usize>() / 16_000);
        let mut state = self.ctx.create_state().map_err(|e| TranscribeError::Unavailable(e.to_string()))?;
        // whisper.cpp aborts the process on a DTW assert when a decode window is under 7 audio frames. Each span is logged
        // before it starts and after it ends, so a new process skips finished spans and redoes a crashed one on token times.
        let (mut done, mut crashed) = (0usize, None);
        if let Some(path) = &self.cfg.progress {
            for line in std::fs::read_to_string(path).unwrap_or_default().lines() {
                let v: serde_json::Value = serde_json::from_str(line).map_err(|e| TranscribeError::Provider(e.to_string()))?;
                let i = v["i"].as_u64().unwrap_or(0) as usize;
                match v["words"].as_array() {
                    Some(ws) => {
                        words.extend(ws.iter().map(|w| Word { text: w["w"].as_str().unwrap_or("").into(), start_ms: w["s"].as_i64().unwrap_or(0), end_ms: w["e"].as_i64().unwrap_or(0), speaker: None }));
                        done = i + 1;
                        crashed = None;
                    }
                    None => crashed = Some(i),
                }
            }
        }
        let log = |line: String| {
            if let Some(path) = &self.cfg.progress {
                use std::io::Write;
                let mut f = std::fs::OpenOptions::new().create(true).append(true).open(path).unwrap();
                writeln!(f, "{line}").unwrap();
            }
        };
        for (i, (a, b)) in parts.into_iter().enumerate().skip(done) {
            usage.calls += 1;
            log(serde_json::json!({"i": i}).to_string());
            let before = words.len();
            let base = (a as i64) / 16;
            if crashed == Some(i) {
                eprintln!("span {i} at {base} ms crashed before, redoing it on token times");
                usage.failed_chunks += 1;
                let plain = WhisperContext::new_with_params(&self.cfg.model, { let mut p = WhisperContextParameters::default(); p.use_gpu(self.cfg.gpu); p }).map_err(|e| TranscribeError::Unavailable(e.to_string()))?;
                let mut st = plain.create_state().map_err(|e| TranscribeError::Unavailable(e.to_string()))?;
                self.run(&mut st, &plain, false, &pcm[a..b], language, base, &mut words)?;
            } else {
                self.run(&mut state, &self.ctx, self.cfg.dtw.is_some(), &pcm[a..b], language, base, &mut words)?;
            }
            let span: Vec<_> = words[before..].iter().map(|w| serde_json::json!({"w": w.text, "s": w.start_ms, "e": w.end_ms})).collect();
            log(serde_json::json!({"i": i, "words": span}).to_string());
        }
        Ok(Transcript { words, usage })
    }
}
