//! Decoding a recording to 16 kHz mono PCM in a file. Samples are placed on the file's own timeline from packet timestamps, as anchors point into the original: a late start and holes in the packets have to survive as silence. It holds a packet and a resampling window in memory, whatever the length.
use crate::resample::Resampler;
use memex_core::meeting::PCM_RATE;
use memex_types::MediaFacts;
use opus_rs::OpusDecoder;
use std::fmt;
use std::fs::File;
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;
use symphonia::core::codecs::CodecParameters;
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::codecs::audio::well_known::CODEC_ID_OPUS;
use symphonia::core::errors::Error;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

/// A hole or overlap smaller than this is timestamp rounding, so the stream stays continuous (spec 027).
const TOLERANCE_MS: f64 = 30.0;
/// More undecodable packets than this means audio has gone, which mustn't pass as a short transcript. Symphonia does this silently on an MP3 that changes channels mid-file (spec 027).
const MAX_BAD_PACKET_SHARE: f64 = 0.01;
/// The largest `moov` we read looking for an edit list.
const MAX_MOOV_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    /// The file can't be used, and trying again won't help.
    Refused(String),
    /// Our own disk failed.
    Io(String),
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused(m) | Self::Io(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for DecodeError {}

fn refused(message: impl Into<String>) -> DecodeError {
    DecodeError::Refused(message.into())
}

fn io(e: std::io::Error) -> DecodeError {
    DecodeError::Io(e.to_string())
}

/// Places mono samples on the timeline and writes them out at 16 kHz, stopping when the recording passes the limit.
struct Timeline {
    rate: u32,
    resampler: Resampler,
    out: BufWriter<File>,
    buffer: Vec<f32>,
    /// Source samples placed so far.
    placed: u64,
    max_placed: u64,
    written: u64,
    max_seconds: u64,
}

impl Timeline {
    fn flush(&mut self) -> Result<(), DecodeError> {
        for s in &self.buffer {
            self.out.write_all(&s.to_le_bytes()).map_err(io)?;
        }
        self.written += self.buffer.len() as u64;
        self.buffer.clear();
        Ok(())
    }

    fn over(&self) -> DecodeError {
        refused(format!("the recording is longer than the limit of {} s", self.max_seconds))
    }

    fn push(&mut self, mono: &[f32]) -> Result<(), DecodeError> {
        self.placed += mono.len() as u64;
        if self.placed > self.max_placed {
            return Err(self.over());
        }
        self.resampler.push(mono, &mut self.buffer);
        self.flush()
    }

    /// Silence, a piece at a time, so a packet claiming to be hours later can't take memory, and fails at the limit.
    fn silence(&mut self, mut samples: u64) -> Result<(), DecodeError> {
        if self.placed.saturating_add(samples) > self.max_placed {
            return Err(self.over());
        }
        let zeros = [0f32; 4096];
        while samples > 0 {
            let n = samples.min(zeros.len() as u64) as usize;
            self.push(&zeros[..n])?;
            samples -= n as u64;
        }
        Ok(())
    }
}

enum Dec {
    Opus(Box<OpusDecoder>, Vec<f32>),
    Other(Box<dyn symphonia::core::codecs::audio::AudioDecoder>),
}

/// Decodes `path` to little-endian `f32` samples at 16 kHz in `out`. What it found comes back. Damage that loses audio is refused.
pub fn decode_to_file(path: &Path, out: &Path, max_seconds: u64) -> Result<MediaFacts, DecodeError> {
    let file = File::open(path).map_err(io)?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let hint = Hint::new();
    let mut format = symphonia::default::get_probe()
        .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
        .map_err(|e| refused(format!("not a readable recording: {e}")))?;
    let track = format.default_track(TrackType::Audio).ok_or_else(|| refused("the file has no audio track"))?;
    let track_id = track.id;
    let time_base = track.time_base.ok_or_else(|| refused("the audio track has no time base"))?;
    let params = match track.codec_params.as_ref() {
        Some(CodecParameters::Audio(a)) => a.clone(),
        _ => return Err(refused("the audio track has no audio parameters")),
    };
    let seconds_per_tick = f64::from(time_base.numer.get()) / f64::from(time_base.denom.get());
    let track_delay = track.delay.unwrap_or(0) as usize;
    let is_opus = params.codec == CODEC_ID_OPUS;
    let head_channels = params
        .extra_data
        .as_ref()
        .filter(|d| d.len() > 9 && d.starts_with(b"OpusHead"))
        .map(|d| usize::from(d[9]));
    let channels_hint = head_channels.or(params.channels.as_ref().map(|c| c.count())).unwrap_or(2);
    let mut dec = if is_opus {
        let c = channels_hint.clamp(1, 2);
        Dec::Opus(
            Box::new(OpusDecoder::new(48_000, c).map_err(|e| refused(format!("opus: {e}")))?),
            vec![0.0; 5760 * c],
        )
    } else {
        Dec::Other(
            symphonia::default::get_codecs()
                .make_audio_decoder(&params, &AudioDecoderOptions::default())
                .map_err(|e| refused(format!("this codec isn't supported: {e}")))?,
        )
    };
    let edit = mp4_edit(path).map_err(io)?;
    let mut rate = if is_opus { 48_000 } else { params.sample_rate.unwrap_or(0) };
    let codec = if is_opus { "opus".to_string() } else { format!("{:?}", params.codec) };
    let writer = BufWriter::with_capacity(1 << 20, File::create(out).map_err(io)?);
    let mut timeline: Option<Timeline> = None;
    let mut pending_writer = Some(writer);
    let (mut packets, mut bad_packets, mut gaps) = (0u64, 0u64, 0u32);
    let mut gap_samples = 0u64;
    let (mut interleaved, mut mono): (Vec<f32>, Vec<f32>) = (vec![], vec![]);
    let mut channel_count = channels_hint as u32;
    loop {
        let packet = match format.next_packet() {
            Ok(Some(p)) => p,
            // The end of a truncated file. What's there still counts.
            Ok(None) | Err(Error::IoError(_)) | Err(Error::ResetRequired) => break,
            Err(e) => return Err(refused(format!("the file can't be read: {e}"))),
        };
        if packet.track_id != track_id {
            continue;
        }
        packets += 1;
        let channels;
        match &mut dec {
            Dec::Opus(d, buffer) => {
                let c = channels_hint.clamp(1, 2);
                match d.decode(&packet.data, 5760, buffer) {
                    Ok(n) => {
                        interleaved.clear();
                        interleaved.extend_from_slice(&buffer[..n * c]);
                        channels = c;
                    }
                    Err(_) => {
                        bad_packets += 1;
                        continue;
                    }
                }
            }
            Dec::Other(d) => match d.decode(&packet) {
                Ok(b) => {
                    channels = b.spec().channels().count().max(1);
                    if rate == 0 {
                        rate = b.spec().rate();
                    }
                    b.copy_to_vec_interleaved(&mut interleaved);
                }
                Err(Error::DecodeError(_)) => {
                    bad_packets += 1;
                    continue;
                }
                Err(e) => return Err(refused(format!("the audio can't be decoded: {e}"))),
            },
        }
        if rate == 0 {
            return Err(refused("the audio track has no sample rate"));
        }
        channel_count = channels as u32;
        let tl = match &mut timeline {
            Some(t) => t,
            None => timeline.insert(Timeline {
                rate,
                resampler: Resampler::new(rate, PCM_RATE),
                out: pending_writer.take().expect("set once"),
                buffer: Vec::with_capacity(16_384),
                placed: 0,
                max_placed: max_seconds.saturating_mul(u64::from(rate)),
                written: 0,
                max_seconds,
            }),
        };
        // Symphonia's decoders drop their own trims. The Opus decoder is ours, so we do it here.
        let mut frames = interleaved.len() / channels;
        let mut start = 0usize;
        if is_opus {
            let to_rate = |ticks: u64| (ticks as f64 * seconds_per_tick * f64::from(rate)).round() as usize;
            start = to_rate(packet.trim_start.get()).min(frames);
            // Ogg reports the pre-skip as track delay and leaves the first packet untrimmed. WebM's first timestamp is already negative.
            if packets == 1 && start == 0 && packet.pts.get() >= 0 {
                start = track_delay.min(frames);
            }
            frames = frames.saturating_sub(to_rate(packet.trim_end.get())).max(start);
        }
        let pts = packet.pts.get() as f64 * seconds_per_tick;
        let mut position = (pts * f64::from(tl.rate)).round() as i64;
        if let Some((empty_s, skip_s)) = edit {
            position += ((empty_s - skip_s) * f64::from(tl.rate)).round() as i64;
        }
        let have = tl.placed as i64;
        let tolerance = (TOLERANCE_MS / 1000.0 * f64::from(tl.rate)) as i64;
        let mut skip_front = 0usize;
        if position > have + tolerance {
            let hole = (position - have) as u64;
            if have > 0 {
                gaps += 1;
                gap_samples += hole;
            }
            tl.silence(hole)?;
        } else if position + tolerance < have {
            // Overlapping packets keep what's already placed.
            skip_front = (have - position) as usize;
        } else if position < 0 && have == 0 {
            skip_front = (-position) as usize;
        }
        mono.clear();
        for f in (start + skip_front)..frames {
            let s = &interleaved[f * channels..(f + 1) * channels];
            mono.push(s.iter().sum::<f32>() / channels as f32);
        }
        tl.push(&mono)?;
    }
    if packets > 0 && bad_packets as f64 > packets as f64 * MAX_BAD_PACKET_SHARE {
        return Err(refused(format!("{bad_packets} of {packets} packets can't be decoded, so audio would be lost")));
    }
    let Some(mut tl) = timeline else {
        return Err(refused("the file holds no audio that can be decoded"));
    };
    if tl.placed == 0 {
        return Err(refused("the file holds no audio that can be decoded"));
    }
    let mut last = vec![];
    tl.resampler.finish(&mut last);
    tl.buffer.append(&mut last);
    tl.flush()?;
    tl.out.flush().map_err(io)?;
    let duration_ms = (tl.written * 1000 / u64::from(PCM_RATE)) as i64;
    Ok(MediaFacts {
        codec,
        sample_rate: rate,
        channels: channel_count,
        duration_ms,
        gaps,
        gap_ms: (gap_samples * 1000 / u64::from(rate)) as i64,
        packets,
        bad_packets,
        skipped_spans: 0,
        coarse_spans: 0,
    })
}

fn be32(b: &[u8]) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(..4)?.try_into().ok()?))
}

fn be64(b: &[u8]) -> Option<u64> {
    Some(u64::from_be_bytes(b.get(..8)?.try_into().ok()?))
}

fn next_box(b: &[u8]) -> Option<(&[u8], &[u8], &[u8])> {
    let (mut size, mut header) = (be32(b)? as usize, 8);
    if size == 1 {
        size = usize::try_from(be64(b.get(8..)?)?).ok()?;
        header = 16;
    } else if size == 0 {
        size = b.len();
    }
    if size < header || size > b.len() {
        return None;
    }
    Some((b.get(4..8)?, &b[header..size], &b[size..]))
}

fn child<'a>(mut b: &'a [u8], want: &[u8; 4]) -> Option<&'a [u8]> {
    while let Some((kind, body, next)) = next_box(b) {
        if kind == want {
            return Some(body);
        }
        b = next;
    }
    None
}

/// Walks the top-level boxes by seeking, and reads only `moov`, so a large video stays out of memory.
fn read_moov(f: &mut File) -> std::io::Result<Option<Vec<u8>>> {
    let total = f.metadata()?.len();
    let mut at = 0u64;
    let mut head = [0u8; 16];
    while at + 8 <= total {
        f.seek(SeekFrom::Start(at))?;
        let got = f.read(&mut head)?;
        if got < 8 {
            return Ok(None);
        }
        let mut size = u64::from(u32::from_be_bytes([head[0], head[1], head[2], head[3]]));
        let mut header = 8u64;
        if size == 1 {
            if got < 16 {
                return Ok(None);
            }
            size = u64::from_be_bytes([head[8], head[9], head[10], head[11], head[12], head[13], head[14], head[15]]);
            header = 16;
        } else if size == 0 {
            size = total - at;
        }
        if size < header {
            return Ok(None);
        }
        if &head[4..8] == b"moov" {
            let len = size - header;
            if len > MAX_MOOV_BYTES {
                return Ok(None);
            }
            f.seek(SeekFrom::Start(at + header))?;
            let mut body = Vec::with_capacity(len as usize);
            f.take(len).read_to_end(&mut body)?;
            return Ok(Some(body));
        }
        at = at.saturating_add(size);
    }
    Ok(None)
}

/// Pure: an audio track's edit list from a `moov`, as (leading empty edit, media time skipped) in seconds. None when it isn't there or doesn't parse.
pub fn edit_from_moov(moov: &[u8]) -> Option<(f64, f64)> {
    let version_at = |b: &[u8]| -> Option<f64> { Some(f64::from(if *b.first()? == 1 { be32(b.get(20..)?)? } else { be32(b.get(12..)?)? })) };
    let movie_scale = version_at(child(moov, b"mvhd")?)?;
    let mut rest = moov;
    while let Some((kind, body, next)) = next_box(rest) {
        rest = next;
        if kind != b"trak" {
            continue;
        }
        let mdia = child(body, b"mdia")?;
        if child(mdia, b"hdlr")?.get(8..12)? != b"soun" {
            continue;
        }
        let media_scale = version_at(child(mdia, b"mdhd")?)?;
        if movie_scale == 0.0 || media_scale == 0.0 {
            return None;
        }
        let Some(elst) = child(body, b"edts").and_then(|e| child(e, b"elst")) else {
            return Some((0.0, 0.0));
        };
        let wide = *elst.first()? == 1;
        let entries = be32(elst.get(4..)?)? as usize;
        let (mut empty, mut skip, mut p) = (0.0, 0.0, 8usize);
        for _ in 0..entries {
            let (duration, time) = if wide {
                (be64(elst.get(p..)?)? as f64, be64(elst.get(p + 8..)?)? as i64)
            } else {
                (f64::from(be32(elst.get(p..)?)?), i64::from(be32(elst.get(p + 4..)?)? as i32))
            };
            p += if wide { 20 } else { 12 };
            if time < 0 {
                empty += duration / movie_scale;
            } else {
                skip = time as f64 / media_scale;
                break;
            }
        }
        return Some((empty, skip));
    }
    None
}

/// Symphonia's MP4 reader ignores edit lists, and without them M4A is 23 ms off, MP4 46 ms, and late audio loses all its lateness (spec 027). Anything that isn't MP4 has none.
fn mp4_edit(path: &Path) -> std::io::Result<Option<(f64, f64)>> {
    let mut f = File::open(path)?;
    let mut head = [0u8; 8];
    if f.read(&mut head)? < 8 || &head[4..8] != b"ftyp" {
        return Ok(None);
    }
    Ok(read_moov(&mut f)?.as_deref().and_then(edit_from_moov))
}

/// A WAV file's bytes, for tests that need a recording.
#[cfg(test)]
pub(crate) fn wav_for_tests(rate: u32, channels: u16, samples: &[i16]) -> Vec<u8> {
    tests::wav_bytes(rate, channels, samples)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bx(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut o = ((body.len() + 8) as u32).to_be_bytes().to_vec();
        o.extend_from_slice(kind);
        o.extend_from_slice(body);
        o
    }

    fn header(scale: u32) -> Vec<u8> {
        let mut b = vec![0u8; 12];
        b.extend_from_slice(&scale.to_be_bytes());
        b.extend_from_slice(&[0u8; 8]);
        b
    }

    fn moov(entries: &[(u32, i32)]) -> Vec<u8> {
        let mut elst = vec![0, 0, 0, 0];
        elst.extend_from_slice(&(entries.len() as u32).to_be_bytes());
        for (d, t) in entries {
            elst.extend_from_slice(&d.to_be_bytes());
            elst.extend_from_slice(&t.to_be_bytes());
            elst.extend_from_slice(&[0, 1, 0, 0]);
        }
        let mut hdlr = vec![0u8; 8];
        hdlr.extend_from_slice(b"soun");
        let mdia = [bx(b"mdhd", &header(48_000)), bx(b"hdlr", &hdlr)].concat();
        let trak = [bx(b"edts", &bx(b"elst", &elst)), bx(b"mdia", &mdia)].concat();
        [bx(b"mvhd", &header(1000)), bx(b"trak", &trak)].concat()
    }

    #[test]
    fn an_edit_list_gives_the_delay_and_the_skip() {
        let (empty, skip) = edit_from_moov(&moov(&[(1500, -1), (9000, 1024)])).unwrap();
        assert!((empty - 1.5).abs() < 1e-9);
        assert!((skip - 1024.0 / 48_000.0).abs() < 1e-9);
    }

    #[test]
    fn damaged_boxes_give_nothing_and_never_panic() {
        let good = moov(&[(1500, -1), (9000, 1024)]);
        for cut in 0..good.len() {
            let _ = edit_from_moov(&good[..cut]);
        }
        let mut flipped = good.clone();
        for i in 0..flipped.len() {
            flipped[i] ^= 0xff;
            let _ = edit_from_moov(&flipped);
            flipped[i] ^= 0xff;
        }
        assert_eq!(edit_from_moov(b"garbage"), None);
    }

    fn temp(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("memex-decode-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    pub(crate) fn wav_bytes(rate: u32, channels: u16, samples: &[i16]) -> Vec<u8> {
        let bytes = (samples.len() * 2) as u32;
        let mut o = b"RIFF".to_vec();
        o.extend_from_slice(&(36 + bytes).to_le_bytes());
        o.extend_from_slice(b"WAVEfmt ");
        o.extend_from_slice(&16u32.to_le_bytes());
        o.extend_from_slice(&1u16.to_le_bytes());
        o.extend_from_slice(&channels.to_le_bytes());
        o.extend_from_slice(&rate.to_le_bytes());
        o.extend_from_slice(&(rate * u32::from(channels) * 2).to_le_bytes());
        o.extend_from_slice(&(channels * 2).to_le_bytes());
        o.extend_from_slice(&16u16.to_le_bytes());
        o.extend_from_slice(b"data");
        o.extend_from_slice(&bytes.to_le_bytes());
        for s in samples {
            o.extend_from_slice(&s.to_le_bytes());
        }
        o
    }

    fn pcm_of(path: &Path) -> Vec<f32> {
        std::fs::read(path).unwrap().as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect()
    }

    #[test]
    fn a_wav_decodes_to_mono_at_16_khz_with_its_timing() {
        let d = temp("wav");
        // Stereo at 48 kHz: 1 s of silence, then a tone for 1 s.
        let mut samples = vec![0i16; 96_000];
        samples.extend((0..48_000).flat_map(|i| {
            let v = ((i as f32 * 0.1).sin() * 20_000.0) as i16;
            [v, v]
        }));
        std::fs::write(d.join("a"), wav_bytes(48_000, 2, &samples)).unwrap();
        let facts = decode_to_file(&d.join("a"), &d.join("pcm"), 3600).unwrap();
        assert_eq!((facts.sample_rate, facts.channels), (48_000, 2));
        assert!((facts.duration_ms - 2000).abs() <= 2, "{}", facts.duration_ms);
        let pcm = pcm_of(&d.join("pcm"));
        let first = pcm.iter().position(|s| s.abs() > 0.05).unwrap();
        assert!((first as i64 - 16_000).abs() < 40, "{first}");
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn a_recording_over_the_limit_is_refused() {
        let d = temp("long");
        std::fs::write(d.join("a"), wav_bytes(16_000, 1, &vec![100i16; 16_000 * 3])).unwrap();
        let e = decode_to_file(&d.join("a"), &d.join("pcm"), 2).unwrap_err();
        assert!(matches!(&e, DecodeError::Refused(m) if m.contains("limit")), "{e}");
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn what_isnt_a_recording_is_refused_and_does_not_panic() {
        let d = temp("bad");
        let good = wav_bytes(16_000, 1, &vec![100i16; 16_000]);
        let mut cases: Vec<Vec<u8>> = vec![
            vec![],
            b"not audio at all, just some text".to_vec(),
            vec![0u8; 4096],
            vec![0xff; 4096],
            good[..30].to_vec(),
        ];
        cases.push(b"RIFF\xff\xff\xff\xffWAVEfmt ".to_vec());
        for (i, case) in cases.iter().enumerate() {
            std::fs::write(d.join("a"), case).unwrap();
            assert!(
                matches!(decode_to_file(&d.join("a"), &d.join("pcm"), 3600), Err(DecodeError::Refused(_))),
                "case {i}"
            );
        }
        std::fs::remove_dir_all(d).unwrap();
    }
}
