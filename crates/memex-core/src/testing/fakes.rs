//! Fakes for tests in this workspace. Tests never use the production wiring.
use crate::{Clock, Seed};
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::time::Duration;

/// A rasterizer that remembers what it was asked for in the bytes it returns, so a test can check the wiring with no pdfium.
#[derive(Debug, Default)]
pub struct FakeRasterizer;

impl crate::Rasterizer for FakeRasterizer {
    fn renderer(&self) -> String {
        "fake-renderer".into()
    }

    fn render(&self, _: &[u8], page: u32, dpi: u32, highlights: &[memex_types::Rect]) -> Result<crate::PageImage, crate::RasterError> {
        Ok(crate::PageImage {
            png: format!("fake page {page} at {dpi} with {} highlights", highlights.len()).into_bytes(),
            width: 10,
            height: 10,
        })
    }
}

/// A reranker that scores a passage by how many of the query's words it holds, counts its calls and fails on request.
#[derive(Debug, Default)]
pub struct FakeReranker {
    calls: AtomicU64,
    passages: AtomicU64,
    fail: std::sync::atomic::AtomicBool,
}

impl FakeReranker {
    pub fn calls(&self) -> u64 {
        self.calls.load(Ordering::SeqCst)
    }

    pub fn passages(&self) -> u64 {
        self.passages.load(Ordering::SeqCst)
    }

    pub fn fail(&self, on: bool) {
        self.fail.store(on, Ordering::SeqCst);
    }
}

impl crate::Reranker for FakeReranker {
    fn model(&self) -> String {
        "fake-reranker".into()
    }

    fn rerank(&self, query: &str, passages: &[&str]) -> Result<Vec<f32>, memex_types::RerankError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.passages.fetch_add(passages.len() as u64, Ordering::SeqCst);
        if self.fail.load(Ordering::SeqCst) {
            return Err(memex_types::RerankError::Failed("a scripted failure".into()));
        }
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        Ok(passages
            .iter()
            .map(|p| {
                let p = p.to_lowercase();
                words.iter().filter(|w| p.contains(w.as_str())).count() as f32
            })
            .collect())
    }
}

/// A clock tests move by hand, so nothing sleeps.
#[derive(Debug, Default)]
pub struct ManualClock {
    ms: AtomicI64,
}

impl ManualClock {
    pub fn starting_at(ms: i64) -> Self {
        Self { ms: AtomicI64::new(ms) }
    }

    pub fn advance(&self, by: Duration) {
        self.ms.fetch_add(by.as_millis() as i64, Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now_ms(&self) -> i64 {
        self.ms.load(Ordering::SeqCst)
    }

    fn monotonic(&self) -> Duration {
        Duration::from_millis(self.now_ms().max(0) as u64)
    }
}

/// A deterministic sequence (splitmix64), so a test can hardcode what it draws.
#[derive(Debug)]
pub struct SeededRandom {
    state: AtomicU64,
}

impl SeededRandom {
    pub fn new(seed: u64) -> Self {
        Self { state: AtomicU64::new(seed) }
    }
}

impl Seed for SeededRandom {
    fn next_u64(&self) -> u64 {
        let mut z = self
            .state
            .fetch_add(0x9E37_79B9_7F4A_7C15, Ordering::SeqCst)
            .wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manual_clock_moves_only_when_told() {
        let c = ManualClock::starting_at(1000);
        assert_eq!(c.now_ms(), 1000);
        c.advance(Duration::from_millis(250));
        assert_eq!(c.now_ms(), 1250);
        assert_eq!(c.monotonic(), Duration::from_millis(1250));
    }

    #[test]
    fn seeded_random_is_repeatable_and_differs_by_seed() {
        let (a, b, other) = (SeededRandom::new(7), SeededRandom::new(7), SeededRandom::new(8));
        let (x, y) = (a.next_u64(), a.next_u64());
        assert_ne!(x, y);
        assert_eq!((x, y), (b.next_u64(), b.next_u64()));
        assert_ne!(x, other.next_u64());
    }
}

/// A transcriber that hears a word per second of sound over a threshold, named by the second it starts in, and nothing in silence. It counts the samples it was sent, so a test can see what reached it. It never stands in for a model's quality.
#[derive(Debug, Default)]
pub struct FakeTranscriber {
    samples: AtomicU64,
    calls: AtomicU64,
    fail: std::sync::atomic::AtomicBool,
}

impl FakeTranscriber {
    pub fn samples(&self) -> u64 {
        self.samples.load(Ordering::SeqCst)
    }

    pub fn calls(&self) -> u64 {
        self.calls.load(Ordering::SeqCst)
    }

    pub fn fail(&self, on: bool) {
        self.fail.store(on, Ordering::SeqCst);
    }
}

impl crate::Transcriber for FakeTranscriber {
    fn model(&self) -> String {
        "fake-transcriber".into()
    }

    fn transcribe(&self, pcm: &[f32], _: Option<&str>) -> Result<crate::Transcribed, crate::TranscribeError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.samples.fetch_add(pcm.len() as u64, Ordering::SeqCst);
        if self.fail.load(Ordering::SeqCst) {
            return Err(crate::TranscribeError::Unavailable("a scripted failure".into()));
        }
        let rate = crate::meeting::PCM_RATE as usize;
        let words = pcm
            .chunks(rate)
            .enumerate()
            .filter(|(_, second)| second.iter().any(|s| s.abs() > 0.01))
            .map(|(i, _)| memex_types::TimedWord {
                text: format!("word{i}"),
                start_ms: i as i64 * 1000,
                end_ms: i as i64 * 1000 + 500,
                speaker: None,
            })
            .collect();
        Ok(crate::Transcribed {
            words,
            usage: memex_types::TranscribeUsage::default(),
        })
    }
}
