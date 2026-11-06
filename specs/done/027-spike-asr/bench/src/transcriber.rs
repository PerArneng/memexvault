//! The `Transcriber` sketch for spec 028: plain types, no whisper or HTTP type crosses it.
#[derive(Clone, Debug)]
pub struct Word {
    pub text: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub speaker: Option<String>,
}
#[derive(Debug, Default)]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub calls: u64,
    pub failed_chunks: u64,
}
#[derive(Debug)]
pub enum TranscribeError {
    Unavailable(String),
    Provider(String),
}
pub struct Transcript {
    pub words: Vec<Word>,
    pub usage: Usage,
}
/// Takes 16 kHz mono. `language` is a hint such as "sv". Word times are relative to the start of `pcm`.
pub trait Transcriber {
    fn transcribe(&self, pcm: &[f32], language: &str) -> Result<Transcript, TranscribeError>;
}

/// Cuts at the quietest 200 ms within the last 30 s before each `target_s`, so no word is cut in half.
pub fn chunks(pcm: &[f32], target_s: usize) -> Vec<(usize, usize)> {
    const R: usize = 16_000;
    let (mut out, mut a, target) = (Vec::new(), 0usize, target_s * R);
    while pcm.len() - a > target + 30 * R {
        let lo = a + target - 30 * R;
        let hop = R / 5;
        let (mut best, mut best_e) = (a + target, f32::MAX);
        let mut p = lo;
        while p + hop <= a + target {
            let e: f32 = pcm[p..p + hop].iter().map(|v| v * v).sum();
            if e < best_e {
                best_e = e;
                best = p + hop / 2;
            }
            p += hop;
        }
        out.push((a, best));
        a = best;
    }
    out.push((a, pcm.len()));
    out
}
