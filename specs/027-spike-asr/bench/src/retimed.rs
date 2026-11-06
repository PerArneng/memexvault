//! API text and speakers on a local model's clock. The API's own word times aren't usable, see the Result in spec.md.
use crate::transcriber::*;
use similar::{capture_diff_slices, Algorithm, DiffOp};

fn norm(w: &str) -> String {
    w.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// Matched words take the clock's times, and words between matches share the time between them by length. Returns the matched share.
pub fn retime(api: &mut [Word], clock: &[Word]) -> f64 {
    let (a, c): (Vec<_>, Vec<_>) = (api.iter().map(|w| norm(&w.text)).collect(), clock.iter().map(|w| norm(&w.text)).collect());
    let mut matched = vec![false; api.len()];
    let mut n = 0;
    for op in capture_diff_slices(Algorithm::Myers, &a, &c) {
        // A lone equal word between edits is often a chance match such as "the".
        if let DiffOp::Equal { old_index, new_index, len } = op {
            if len >= 2 {
                for k in 0..len {
                    api[old_index + k].start_ms = clock[new_index + k].start_ms;
                    api[old_index + k].end_ms = clock[new_index + k].end_ms;
                    matched[old_index + k] = true;
                    n += 1;
                }
            }
        }
    }
    let anchors: Vec<usize> = (0..api.len()).filter(|&i| matched[i]).collect();
    for pair in anchors.windows(2) {
        let (lo, hi) = (pair[0], pair[1]);
        if hi - lo < 2 {
            continue;
        }
        let (t0, t1) = (api[lo].end_ms, api[hi].start_ms);
        let total: usize = (lo + 1..hi).map(|i| a[i].chars().count().max(1)).sum();
        let mut used = 0;
        for i in lo + 1..hi {
            api[i].start_ms = t0 + (t1 - t0) * used as i64 / total as i64;
            used += a[i].chars().count().max(1);
            api[i].end_ms = t0 + (t1 - t0) * used as i64 / total as i64;
        }
    }
    // Words outside the first and last match keep their offset to the nearest one.
    if let (Some(&first), Some(&last)) = (anchors.first(), anchors.last()) {
        // The anchors already moved, so shift by the distance from each edge word's old neighbour.
        for i in (0..first).rev() {
            let d = (api[i + 1].start_ms - api[i].start_ms).max(0).min(2000);
            let len = (api[i].end_ms - api[i].start_ms).max(0);
            api[i].start_ms = api[i + 1].start_ms - d.max(len).min(2000);
            api[i].end_ms = api[i].start_ms + len;
        }
        for i in last + 1..api.len() {
            let len = (api[i].end_ms - api[i].start_ms).max(0);
            api[i].start_ms = api[i - 1].end_ms;
            api[i].end_ms = api[i].start_ms + len;
        }
    }
    n as f64 / api.len().max(1) as f64
}

pub struct Retimed {
    pub api: Box<dyn Transcriber>,
    pub clock: Box<dyn Transcriber>,
}
impl Transcriber for Retimed {
    fn transcribe(&self, pcm: &[f32], language: &str) -> Result<Transcript, TranscribeError> {
        // The local run comes first: it costs nothing, and it fails before any money is spent.
        let clock = self.clock.transcribe(pcm, language)?;
        let mut t = self.api.transcribe(pcm, language)?;
        let share = retime(&mut t.words, &clock.words);
        eprintln!("retimed: {:.1}% of API words matched the clock", share * 100.0);
        if share < 0.5 {
            return Err(TranscribeError::Provider(format!("only {:.0}% of words matched the clock", share * 100.0)));
        }
        Ok(t)
    }
}
