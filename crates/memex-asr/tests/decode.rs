//! Decoding each container on small synthetic tones, with times against what was generated. `tests/fixtures/README.md` says how they were made.
// Reads its fixtures from disk.
#![allow(clippy::disallowed_methods)]
use memex_asr::decode::{DecodeError, decode_to_file};
use std::path::PathBuf;

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

fn temp(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("memex-decode-it-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn pcm_of(path: &std::path::Path) -> Vec<f32> {
    std::fs::read(path).unwrap().as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect()
}

/// When the tone starts, in ms. It's steady, so the first loud 10 ms show it.
fn onset_ms(pcm: &[f32]) -> i64 {
    let hop = 160;
    let first = pcm
        .chunks(hop)
        .position(|c| c.iter().map(|s| s.abs()).fold(0.0, f32::max) > 0.1)
        .expect("a tone");
    first as i64 * 10
}

#[test]
fn each_container_decodes_with_the_tone_where_it_should_be() {
    for name in ["tone.wav", "tone.mp3", "tone.m4a", "tone.opus", "tone.webm", "tone.flac", "tone.mp4"] {
        let d = temp(name);
        let facts = decode_to_file(&fixture(name), &d.join("pcm"), 3600).unwrap_or_else(|e| panic!("{name}: {e}"));
        let pcm = pcm_of(&d.join("pcm"));
        let at = onset_ms(&pcm);
        assert!((at - 1000).abs() <= 40, "{name}: the tone starts at {at} ms");
        assert!((facts.duration_ms - 2000).abs() <= 80, "{name}: {} ms", facts.duration_ms);
        assert_eq!(pcm.len() as i64 / 16, facts.duration_ms, "{name}");
        assert_eq!(facts.bad_packets, 0, "{name}");
        std::fs::remove_dir_all(d).unwrap();
    }
}

#[test]
fn audio_that_starts_late_stays_late() {
    let d = temp("late");
    decode_to_file(&fixture("late.m4a"), &d.join("pcm"), 3600).unwrap();
    let at = onset_ms(&pcm_of(&d.join("pcm")));
    assert!((at - 2500).abs() <= 60, "the tone starts at {at} ms");
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn a_video_with_no_audio_is_refused() {
    let d = temp("noaudio");
    let e = decode_to_file(&fixture("no_audio.mp4"), &d.join("pcm"), 3600).unwrap_err();
    assert!(matches!(e, DecodeError::Refused(_)), "{e}");
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn losing_audio_silently_is_refused() {
    // Symphonia rejects every packet after the channels change, which would otherwise pass as a recording half as long.
    let d = temp("channels");
    let e = decode_to_file(&fixture("mono_then_stereo.mp3"), &d.join("pcm"), 3600);
    match e {
        Err(DecodeError::Refused(m)) => assert!(m.contains("packets"), "{m}"),
        // A later Symphonia might decode it. Then all of it has to be there.
        Ok(facts) => assert!(facts.duration_ms > 15_000, "{facts:?}"),
        Err(other) => panic!("{other}"),
    }
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn a_long_silence_claimed_by_a_packet_stops_at_the_limit() {
    let d = temp("limit");
    let e = decode_to_file(&fixture("tone.wav"), &d.join("pcm"), 1).unwrap_err();
    assert!(matches!(&e, DecodeError::Refused(m) if m.contains("limit")), "{e}");
    std::fs::remove_dir_all(d).unwrap();
}

/// Damage can fail a job or give less audio. It can't panic, whichever container it's in.
#[test]
fn truncated_and_flipped_files_never_panic() {
    let d = temp("damage");
    let (mut refused, mut decoded) = (0, 0);
    for name in ["tone.wav", "tone.mp3", "tone.m4a", "tone.opus", "tone.webm", "tone.flac", "tone.mp4"] {
        let bytes = std::fs::read(fixture(name)).unwrap();
        let mut cases: Vec<Vec<u8>> = vec![];
        for keep in [0, 1, 8, 32, bytes.len() / 4, bytes.len() / 2, bytes.len() - 1] {
            cases.push(bytes[..keep].to_vec());
        }
        let mut x = 0x2545f4914f6cdd1du64;
        for _ in 0..24 {
            let mut flipped = bytes.clone();
            for _ in 0..16 {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                let at = (x % flipped.len() as u64) as usize;
                flipped[at] ^= (x >> 32) as u8 | 1;
            }
            cases.push(flipped);
        }
        for case in cases {
            std::fs::write(d.join("a"), &case).unwrap();
            match decode_to_file(&d.join("a"), &d.join("pcm"), 60) {
                Ok(_) => decoded += 1,
                Err(_) => refused += 1,
            }
        }
    }
    assert!(refused > 0 && decoded > 0, "{refused} {decoded}");
    std::fs::remove_dir_all(d).unwrap();
}
