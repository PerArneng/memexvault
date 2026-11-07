//! What every `Transcriber` promises. It runs against the fake and, with its model files, the local implementation. It says nothing about accuracy.
use crate::meeting::PCM_RATE;
use crate::{Transcribed, Transcriber};

fn silence(seconds: usize) -> Vec<f32> {
    vec![0.0; seconds * PCM_RATE as usize]
}

/// Runs on `speech`, samples that hold speech to this transcriber, which go in as it is given them.
pub fn run<T: Transcriber>(t: &T, speech: &[f32], language: Option<&str>) {
    assert!(!t.model().is_empty(), "it names its model");
    let heard: Transcribed = t.transcribe(speech, language).expect("speech transcribes");
    assert!(!heard.words.is_empty(), "it hears the speech");
    let length_ms = speech.len() as i64 * 1000 / i64::from(PCM_RATE);
    let mut last = 0;
    for w in &heard.words {
        assert!(!w.text.trim().is_empty(), "a word has text");
        assert!(w.start_ms >= 0 && w.start_ms <= length_ms + 1000, "{w:?} is inside the audio of {length_ms} ms");
        assert!(w.end_ms >= w.start_ms, "{w:?} ends after it starts");
        assert!(w.start_ms + 1000 >= last, "{w:?} comes in order");
        last = w.start_ms;
    }
    // Times are relative to the slice it was given, so the same sound later gives later times only from the caller's side.
    let again = t.transcribe(speech, language).expect("it can be used twice");
    assert!(
        again.words.first().is_some_and(|w| w.start_ms <= heard.words[0].start_ms + 1000),
        "times are relative to what it was given"
    );
    assert!(t.transcribe(&[], language).is_ok_and(|e| e.words.is_empty()), "no audio gives no words");
}

/// What a transcriber promises for silence. Only one that takes VAD outside it can promise this for whisper, so it's separate.
pub fn run_silence<T: Transcriber>(t: &T) {
    assert!(t.transcribe(&silence(3), None).is_ok_and(|e| e.words.is_empty()), "silence gives no words");
}

#[cfg(test)]
mod tests {
    use crate::testing::FakeTranscriber;

    #[test]
    fn the_fake_passes() {
        let t = FakeTranscriber::default();
        let tone: Vec<f32> = (0..16_000 * 3).map(|i| (i as f32 * 0.05).sin()).collect();
        super::run(&t, &tone, Some("en"));
        super::run_silence(&t);
    }
}
