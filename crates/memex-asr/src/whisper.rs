//! The local transcriber: whisper.cpp through `whisper-rs`, with DTW word starts, and Silero VAD called directly.
use crate::worker::{Ears, PcmFile, Span, WorkerFail};
use memex_core::meeting::{PCM_RATE, close_word_ends};
use memex_core::{TranscribeError, Transcribed, Transcriber};
use memex_types::{TimedWord, TranscribeUsage};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use whisper_rs::{
    DtwMode, DtwModelPreset, DtwParameters, FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState, WhisperVadContext,
    WhisperVadContextParams, WhisperVadParams,
};

/// Part of a Rendition's parser name.
pub const MODEL: &str = "ggml-large-v3-turbo-q5_0";
pub const WHISPER_FILE: &str = "whisper/ggml-large-v3-turbo-q5_0.bin";
pub const VAD_FILE: &str = "whisper/ggml-silero-v6.2.0.bin";

/// Shorter audio than this gets an error from whisper and holds no word anyway.
const MIN_SAMPLES: usize = PCM_RATE as usize / 10;
/// VAD reads this much at once, so it holds a few minutes in memory and not the recording.
const VAD_WINDOW_SAMPLES: u64 = 300 * PCM_RATE as u64;
const VAD_OVERLAP_SAMPLES: u64 = PCM_RATE as u64;
/// Spans closer than this join, up to `MAX_SPAN_SAMPLES`, so whisper keeps its context across short pauses (spec 027).
const MERGE_GAP_SAMPLES: u64 = PCM_RATE as u64;
pub(crate) const MAX_SPAN_SAMPLES: u64 = 120 * PCM_RATE as u64;

fn unavailable(e: impl std::fmt::Display) -> TranscribeError {
    TranscribeError::Failed(e.to_string())
}

/// One whisper context. `precise` adds DTW word starts, which can abort the process on some model outputs (spec 027), so the worker falls back to token times for a stretch that did.
pub struct WhisperLocal {
    ctx: WhisperContext,
    state: Mutex<WhisperState>,
    threads: i32,
    precise: bool,
}

impl WhisperLocal {
    pub fn new(model: &Path, threads: usize, precise: bool) -> Result<Self, TranscribeError> {
        let mut params = WhisperContextParameters::default();
        params.use_gpu(true);
        if precise {
            params.dtw_parameters(DtwParameters {
                mode: DtwMode::ModelPreset {
                    model_preset: DtwModelPreset::LargeV3Turbo,
                },
                dtw_mem_size: 1024 * 1024 * 256,
            });
        }
        let ctx = WhisperContext::new_with_params(model, params).map_err(unavailable)?;
        let state = ctx.create_state().map_err(unavailable)?;
        Ok(Self {
            ctx,
            state: Mutex::new(state),
            threads: threads as i32,
            precise,
        })
    }
}

fn flush(current: &mut Option<(Vec<u8>, i64, i64)>, out: &mut Vec<TimedWord>) {
    if let Some((bytes, start_ms, end_ms)) = current.take() {
        let text = String::from_utf8_lossy(&bytes).trim().to_string();
        if !text.is_empty() {
            out.push(TimedWord {
                text,
                start_ms,
                end_ms,
                speaker: None,
            });
        }
    }
}

impl Transcriber for WhisperLocal {
    fn model(&self) -> String {
        format!("whisper-rs-0.16.0+{MODEL}+silero-v6.2.0")
    }

    fn transcribe(&self, pcm: &[f32], language: Option<&str>) -> Result<Transcribed, TranscribeError> {
        if pcm.len() < MIN_SAMPLES {
            return Ok(Transcribed::default());
        }
        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_language(Some(language.unwrap_or("auto")));
        params.set_n_threads(self.threads);
        params.set_token_timestamps(true);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_special(false);
        params.set_print_timestamps(false);
        // Without this, one bad window can repeat itself through the rest of a long file.
        params.set_no_context(true);
        let mut state = self.state.lock().expect("lock");
        state.full(params, pcm).map_err(unavailable)?;
        let eot = self.ctx.token_eot();
        let mut words = vec![];
        for segment in state.as_iter() {
            let mut current: Option<(Vec<u8>, i64, i64)> = None;
            for i in 0..segment.n_tokens() {
                let Some(token) = segment.get_token(i) else { continue };
                let data = token.token_data();
                if data.id >= eot {
                    continue;
                }
                let bytes = token.to_bytes().map_err(unavailable)?.to_vec();
                let starts_word = bytes.first() == Some(&b' ');
                let start = if self.precise && data.t_dtw >= 0 { data.t_dtw } else { data.t0 } * 10;
                let end = data.t1 * 10;
                match (&mut current, starts_word) {
                    (Some(c), false) => {
                        c.0.extend_from_slice(&bytes);
                        c.2 = end;
                    }
                    _ => {
                        flush(&mut current, &mut words);
                        current = Some((bytes, start, end));
                    }
                }
            }
            flush(&mut current, &mut words);
        }
        if self.precise {
            close_word_ends(&mut words);
        }
        Ok(Transcribed {
            words,
            usage: TranscribeUsage::default(),
        })
    }
}

/// The real ears: Silero for where speech is and whisper for what was said.
pub struct WhisperEars {
    pub model: PathBuf,
    pub vad: PathBuf,
    pub threads: usize,
}

impl Ears for WhisperEars {
    type T = WhisperLocal;

    fn speech_spans(&self, pcm: &PcmFile) -> Result<Vec<Span>, WorkerFail> {
        let mut context = WhisperVadContextParams::new();
        context.set_n_threads(self.threads as i32);
        context.set_use_gpu(false);
        let path = self.vad.to_string_lossy();
        let mut vad = WhisperVadContext::new(&path, context).map_err(|e| WorkerFail::Failed(e.to_string()))?;
        let mut raw: Vec<Span> = vec![];
        let mut at = 0u64;
        while at < pcm.samples() {
            let count = VAD_WINDOW_SAMPLES + VAD_OVERLAP_SAMPLES;
            let window = pcm.read(at, count).map_err(WorkerFail::io)?;
            let mut params = WhisperVadParams::new();
            params.set_speech_pad(200);
            // Long speech with no pause still reaches whisper in pieces, which keeps a span's memory bounded.
            params.set_max_speech_duration((MAX_SPAN_SAMPLES / u64::from(PCM_RATE)) as f32);
            let segments = vad.segments_from_samples(params, &window).map_err(|e| WorkerFail::Failed(e.to_string()))?;
            // Segment times are in centiseconds.
            for s in segments {
                let a = at + (f64::from(s.start) * 160.0) as u64;
                let b = (at + (f64::from(s.end) * 160.0) as u64).min(pcm.samples());
                if b > a {
                    raw.push(Span { start: a, end: b });
                }
            }
            at += VAD_WINDOW_SAMPLES;
        }
        Ok(merge_spans(raw))
    }

    fn precise(&self) -> Result<WhisperLocal, WorkerFail> {
        WhisperLocal::new(&self.model, self.threads, true).map_err(|e| WorkerFail::Failed(e.to_string()))
    }

    fn coarse(&self) -> Result<WhisperLocal, WorkerFail> {
        WhisperLocal::new(&self.model, self.threads, false).map_err(|e| WorkerFail::Failed(e.to_string()))
    }
}

/// Joins what overlaps, and what's close while the result stays short enough. Pure.
pub fn merge_spans(mut raw: Vec<Span>) -> Vec<Span> {
    raw.sort_by_key(|s| s.start);
    let mut out: Vec<Span> = vec![];
    for s in raw {
        match out.last_mut() {
            // An overlap comes from the windows and has to join whatever its size, or a word is heard twice.
            Some(l) if s.start <= l.end => l.end = l.end.max(s.end),
            Some(l) if s.start <= l.end + MERGE_GAP_SAMPLES && s.end - l.start <= MAX_SPAN_SAMPLES => l.end = s.end,
            _ => out.push(s),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(a: u64, b: u64) -> Span {
        Span {
            start: a * 16_000,
            end: b * 16_000,
        }
    }

    #[test]
    fn close_spans_join_up_to_the_maximum_and_overlaps_always_join() {
        assert_eq!(merge_spans(vec![s(5, 6), s(0, 1)]), vec![s(0, 1), s(5, 6)]);
        assert_eq!(merge_spans(vec![s(0, 1), s(2, 3)]), vec![s(0, 3)], "1 s apart");
        assert_eq!(
            merge_spans(vec![s(0, 100), s(101, 130)]),
            vec![s(0, 100), s(101, 130)],
            "it would be over 120 s"
        );
        assert_eq!(merge_spans(vec![s(0, 100), s(99, 130)]), vec![s(0, 130)], "the same speech seen by two windows");
    }
}
