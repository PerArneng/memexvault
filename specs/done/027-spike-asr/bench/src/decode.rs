//! In-process decoding. Samples are placed on the file's own timeline from packet timestamps, because anchors must
//! point into the original media: a late audio start and holes in the packets have to survive as silence.
use opus_rs::OpusDecoder;
use std::fs::File;
use std::path::Path;
use symphonia::core::codecs::audio::well_known::CODEC_ID_OPUS;
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::codecs::CodecParameters;
use symphonia::core::errors::Error;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;

pub struct Decoded {
    pub mono: Vec<f32>,
    pub rate: u32,
    pub channels: usize,
    pub codec: String,
    pub first_pts_ms: f64,
    pub elst_shift_ms: f64,
    pub gaps: u32,
    pub gap_ms: f64,
    pub bad_packets: u32,
    pub packets: u64,
}

/// A hole or overlap smaller than this is timestamp rounding and is ignored, so the sample stream stays continuous.
const TOLERANCE_MS: f64 = 30.0;

pub fn decode(path: &Path, use_elst: bool) -> Result<Decoded, String> {
    let file = File::open(path).map_err(|e| e.to_string())?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let mut format = symphonia::default::get_probe()
        .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
        .map_err(|e| format!("probe: {e}"))?;
    let track = format.default_track(TrackType::Audio).ok_or("no audio track")?;
    let track_id = track.id;
    let tb = track.time_base.ok_or("no time base")?;
    let params = match track.codec_params.as_ref() {
        Some(CodecParameters::Audio(a)) => a.clone(),
        _ => return Err("no audio parameters".into()),
    };
    let tb_secs = tb.numer.get() as f64 / tb.denom.get() as f64;
    let track_delay = track.delay.unwrap_or(0) as usize;
    let is_opus = params.codec == CODEC_ID_OPUS;
    let head_channels = params.extra_data.as_ref().filter(|d| d.len() > 9 && d.starts_with(b"OpusHead")).map(|d| d[9] as usize);

    enum Dec {
        Opus(OpusDecoder, Vec<f32>),
        Other(Box<dyn symphonia::core::codecs::audio::AudioDecoder>),
    }
    let channels_hint = head_channels.or(params.channels.as_ref().map(|c| c.count())).unwrap_or(2);
    let mut dec = if is_opus {
        let c = channels_hint.min(2);
        Dec::Opus(OpusDecoder::new(48_000, c).map_err(|e| format!("opus: {e}"))?, vec![0.0; 5760 * c])
    } else {
        Dec::Other(
            symphonia::default::get_codecs()
                .make_audio_decoder(&params, &AudioDecoderOptions::default())
                .map_err(|e| format!("codec: {e}"))?,
        )
    };
    let elst = if use_elst { mp4_edit(path) } else { None };
    let mut out = Decoded {
        mono: Vec::new(),
        rate: if is_opus { 48_000 } else { params.sample_rate.unwrap_or(0) },
        channels: channels_hint,
        codec: if is_opus { "opus".into() } else { format!("{:?}", params.codec) },
        first_pts_ms: f64::NAN,
        elst_shift_ms: 0.0,
        gaps: 0,
        gap_ms: 0.0,
        bad_packets: 0,
        packets: 0,
    };
    let mut interleaved: Vec<f32> = Vec::new();
    loop {
        let packet = match format.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) => break,
            Err(Error::IoError(_)) => break,
            Err(Error::ResetRequired) => break,
            Err(e) => return Err(format!("read: {e}")),
        };
        if packet.track_id != track_id {
            continue;
        }
        out.packets += 1;
        let channels;
        match &mut dec {
            Dec::Opus(d, buf) => {
                let c = channels_hint.min(2);
                match d.decode(&packet.data, 5760, buf) {
                    Ok(n) => {
                        interleaved.clear();
                        interleaved.extend_from_slice(&buf[..n * c]);
                        channels = c;
                    }
                    Err(_) => {
                        out.bad_packets += 1;
                        continue;
                    }
                }
            }
            Dec::Other(d) => match d.decode(&packet) {
                Ok(b) => {
                    channels = b.spec().channels().count();
                    if out.rate == 0 {
                        out.rate = b.spec().rate();
                    }
                    b.copy_to_vec_interleaved(&mut interleaved);
                }
                Err(Error::DecodeError(_)) => {
                    out.bad_packets += 1;
                    continue;
                }
                Err(e) => return Err(format!("decode: {e}")),
            },
        }
        // Symphonia's decoders already drop trim_start and trim_end. The Opus decoder is ours, so we do it here.
        let mut frames = interleaved.len() / channels;
        let mut start = 0usize;
        if is_opus {
            let to_rate = |d: u64| (d as f64 * tb_secs * out.rate as f64).round() as usize;
            start = to_rate(packet.trim_start.get()).min(frames);
            // Ogg reports the pre-skip as track delay and leaves the first packet untrimmed; WebM's first pts is already negative.
            if out.packets == 1 && start == 0 && packet.pts.get() >= 0 {
                start = track_delay.min(frames);
            }
            frames = frames.saturating_sub(to_rate(packet.trim_end.get())).max(start);
        }
        let pts = packet.pts.get() as f64 * tb_secs;
        if out.first_pts_ms.is_nan() {
            out.first_pts_ms = pts * 1000.0;
        }
        let mut pos = (pts * out.rate as f64).round() as i64;
        if let Some((empty_s, skip_s)) = elst {
            pos += ((empty_s - skip_s) * out.rate as f64).round() as i64;
        }
        let have = out.mono.len() as i64;
        let tol = (TOLERANCE_MS / 1000.0 * out.rate as f64) as i64;
        let mut skip_front = 0usize;
        if pos > have + tol {
            let hole = (pos - have) as usize;
            if have > 0 {
                out.gaps += 1;
                out.gap_ms += hole as f64 * 1000.0 / out.rate as f64;
            }
            out.mono.resize(out.mono.len() + hole, 0.0);
        } else if pos + tol < have {
            skip_front = (have - pos) as usize; // overlapping packets: keep what is already placed
        } else if pos < 0 && have == 0 {
            skip_front = (-pos) as usize;
        }
        for f in (start + skip_front)..frames {
            let s = &interleaved[f * channels..(f + 1) * channels];
            out.mono.push(s.iter().sum::<f32>() / channels as f32);
        }
    }
    if let Some((e, s)) = elst {
        out.elst_shift_ms = (e - s) * 1000.0;
    }
    if out.mono.is_empty() {
        return Err("no audio decoded".into());
    }
    Ok(out)
}

/// Symphonia's MP4 reader ignores edit lists (its source says so), so read the audio track's `elst` directly.
/// Returns (leading empty edit in seconds, media time skipped in seconds).
pub fn mp4_edit(path: &Path) -> Option<(f64, f64)> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    if !["mp4", "m4a", "mov", "m4v"].contains(&ext.as_str()) {
        return None;
    }
    let data = std::fs::read(path).ok()?;
    let moov = child(&data, b"moov")?;
    let mvhd = child(moov, b"mvhd")?;
    let movie_scale = if mvhd[0] == 1 { be32(&mvhd[20..]) } else { be32(&mvhd[12..]) } as f64;
    let mut rest = moov;
    while let Some((kind, body, next)) = next_box(rest) {
        rest = next;
        if kind != b"trak" {
            continue;
        }
        let mdia = child(body, b"mdia")?;
        let hdlr = child(mdia, b"hdlr")?;
        if &hdlr[8..12] != b"soun" {
            continue;
        }
        let mdhd = child(mdia, b"mdhd")?;
        let media_scale = if mdhd[0] == 1 { be32(&mdhd[20..]) } else { be32(&mdhd[12..]) } as f64;
        let Some(elst) = child(body, b"edts").and_then(|e| child(e, b"elst")) else { return Some((0.0, 0.0)) };
        let v1 = elst[0] == 1;
        let n = be32(&elst[4..]) as usize;
        let (mut empty, mut skip, mut p) = (0.0, 0.0, 8);
        for i in 0..n {
            let (dur, time) = if v1 {
                (be64(&elst[p..]) as f64, be64(&elst[p + 8..]) as i64)
            } else {
                (be32(&elst[p..]) as f64, be32(&elst[p + 4..]) as i32 as i64)
            };
            p += if v1 { 20 } else { 12 };
            if time < 0 {
                empty += dur / movie_scale;
            } else if i == 0 || skip == 0.0 {
                skip = time as f64 / media_scale;
                break;
            }
        }
        return Some((empty, skip));
    }
    None
}
fn be32(b: &[u8]) -> u32 { u32::from_be_bytes([b[0], b[1], b[2], b[3]]) }
fn be64(b: &[u8]) -> u64 { ((be32(b) as u64) << 32) | be32(&b[4..]) as u64 }
fn next_box(b: &[u8]) -> Option<(&[u8], &[u8], &[u8])> {
    if b.len() < 8 {
        return None;
    }
    let (mut size, mut hdr) = (be32(b) as usize, 8);
    if size == 1 {
        size = be64(&b[8..]) as usize;
        hdr = 16;
    } else if size == 0 {
        size = b.len();
    }
    if size < hdr || size > b.len() {
        return None;
    }
    Some((&b[4..8], &b[hdr..size], &b[size..]))
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

/// Kaiser-less Blackman windowed sinc. Each output position is computed from its index, so it can't drift.
pub fn resample(x: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to {
        return x.to_vec();
    }
    let ratio = from as f64 / to as f64;
    let cutoff = (1.0 / ratio).min(1.0) * 0.95;
    let half = (16.0 / cutoff).ceil() as i64;
    let n = (x.len() as f64 / ratio).floor() as usize;
    let mut out = Vec::with_capacity(n);
    const PHASES: usize = 512;
    let width = (half * 2 + 1) as usize;
    let mut table = vec![0f32; PHASES * width];
    for p in 0..PHASES {
        let frac = p as f64 / PHASES as f64;
        for k in 0..width {
            let t = k as f64 - half as f64 - frac;
            let u = t / (half as f64 + 1.0);
            let w = 0.42 + 0.5 * (std::f64::consts::PI * u).cos() + 0.08 * (2.0 * std::f64::consts::PI * u).cos();
            let a = std::f64::consts::PI * t * cutoff;
            let s = if a.abs() < 1e-9 { 1.0 } else { a.sin() / a };
            table[p * width + k] = (cutoff * s * w) as f32;
        }
    }
    for i in 0..n {
        let c = i as f64 * ratio;
        let ic = c.floor() as i64;
        let phase = ((c - ic as f64) * PHASES as f64) as usize;
        let row = &table[phase * width..(phase + 1) * width];
        let mut acc = 0f32;
        let lo = ic - half;
        if lo >= 0 && (lo as usize + width) <= x.len() {
            let sl = &x[lo as usize..lo as usize + width];
            for (a, b) in sl.iter().zip(row) {
                acc += a * b;
            }
        } else {
            for (k, h) in row.iter().enumerate() {
                let j = lo + k as i64;
                if j >= 0 && (j as usize) < x.len() {
                    acc += x[j as usize] * h;
                }
            }
        }
        out.push(acc);
    }
    out
}
