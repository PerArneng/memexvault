//! API implementation. Audio goes inline as 16-bit WAV, in chunks small enough for the 20 MB request limit.
//! The key is passed in and never printed. Spending stops at `max_tokens`.
use crate::transcriber::*;
use base64::Engine;
use serde_json::{json, Value};
use std::cell::Cell;
use std::time::Duration;

pub struct GeminiApi {
    pub key: String,
    pub model: String,
    pub chunk_s: usize,
    pub max_tokens: u64,
    pub spent: Cell<u64>,
}
fn wav(pcm: &[f32]) -> Vec<u8> {
    let bytes = (pcm.len() * 2) as u32;
    let mut o = Vec::with_capacity(44 + bytes as usize);
    o.extend_from_slice(b"RIFF");
    o.extend_from_slice(&(36 + bytes).to_le_bytes());
    o.extend_from_slice(b"WAVEfmt ");
    o.extend_from_slice(&16u32.to_le_bytes());
    o.extend_from_slice(&1u16.to_le_bytes());
    o.extend_from_slice(&1u16.to_le_bytes());
    o.extend_from_slice(&16_000u32.to_le_bytes());
    o.extend_from_slice(&32_000u32.to_le_bytes());
    o.extend_from_slice(&2u16.to_le_bytes());
    o.extend_from_slice(&16u16.to_le_bytes());
    o.extend_from_slice(b"data");
    o.extend_from_slice(&bytes.to_le_bytes());
    for s in pcm {
        o.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    o
}
impl GeminiApi {
    fn call(&self, pcm: &[f32], language: &str, usage: &mut Usage) -> Result<Vec<Word>, TranscribeError> {
        let prompt = format!(
            "Transcribe this recording verbatim, in its spoken language ({language}). Return every spoken word in order, one array item per word. \
             For each word give s and e: its start and end time in seconds from the start of this audio, with millisecond precision, \
             and spk: a label such as S1 or S2 that stays the same for the same voice. Return nothing for silence and never invent words."
        );
        let body = json!({
            "contents": [{"parts": [
                {"text": prompt},
                {"inline_data": {"mime_type": "audio/wav", "data": base64::engine::general_purpose::STANDARD.encode(wav(pcm))}}
            ]}],
            "generationConfig": {
                "temperature": 0.0,
                "maxOutputTokens": 65536,
                "responseMimeType": "application/json",
                "responseSchema": {"type": "ARRAY", "items": {"type": "OBJECT",
                    "properties": {"w": {"type": "STRING"}, "s": {"type": "NUMBER"}, "e": {"type": "NUMBER"}, "spk": {"type": "STRING"}},
                    "required": ["w", "s", "e"]}}
            }
        });
        let url = format!("https://generativelanguage.googleapis.com/v1beta/models/{}:generateContent", self.model);
        let agent: ureq::Agent = ureq::Agent::config_builder().timeout_global(Some(Duration::from_secs(600))).http_status_as_error(false).build().into();
        let mut last = String::new();
        for attempt in 0..4 {
            if attempt > 0 {
                std::thread::sleep(Duration::from_secs(10 * attempt));
            }
            usage.calls += 1;
            let mut resp = match agent.post(&url).header("x-goog-api-key", &self.key).send_json(&body) {
                Ok(r) => r,
                Err(e) => {
                    last = format!("transport: {e}");
                    continue;
                }
            };
            let status = resp.status().as_u16();
            let text = resp.body_mut().with_config().limit(200 * 1024 * 1024).read_to_string().map_err(|e| TranscribeError::Provider(e.to_string()))?;
            if status != 200 {
                last = format!("{status}: {}", text.chars().take(300).collect::<String>());
                if status == 429 || status >= 500 {
                    continue;
                }
                return Err(TranscribeError::Provider(last));
            }
            let v: Value = serde_json::from_str(&text).map_err(|e| TranscribeError::Provider(e.to_string()))?;
            let meta = &v["usageMetadata"];
            let (i, o) = (meta["promptTokenCount"].as_u64().unwrap_or(0), meta["candidatesTokenCount"].as_u64().unwrap_or(0) + meta["thoughtsTokenCount"].as_u64().unwrap_or(0));
            usage.input_tokens += i;
            usage.output_tokens += o;
            self.spent.set(self.spent.get() + i + o);
            let finish = v["candidates"][0]["finishReason"].as_str().unwrap_or("?").to_string();
            let reply = v["candidates"][0]["content"]["parts"][0]["text"].as_str().unwrap_or("");
            let Ok(items) = serde_json::from_str::<Vec<Value>>(reply) else {
                last = format!("unparseable reply, finish={finish}, {} chars", reply.len());
                continue;
            };
            return Ok(items
                .iter()
                .filter_map(|w| {
                    Some(Word {
                        text: w["w"].as_str()?.trim().to_string(),
                        start_ms: (w["s"].as_f64()? * 1000.0) as i64,
                        end_ms: (w["e"].as_f64()? * 1000.0) as i64,
                        speaker: w["spk"].as_str().map(str::to_string),
                    })
                })
                .collect());
        }
        Err(TranscribeError::Provider(last))
    }
}
impl Transcriber for GeminiApi {
    fn transcribe(&self, pcm: &[f32], language: &str) -> Result<Transcript, TranscribeError> {
        let (mut words, mut usage) = (Vec::new(), Usage::default());
        for (a, b) in chunks(pcm, self.chunk_s) {
            if self.spent.get() > self.max_tokens {
                return Err(TranscribeError::Provider(format!("token cap of {} reached", self.max_tokens)));
            }
            let base = a as i64 / 16;
            match self.call(&pcm[a..b], language, &mut usage) {
                Ok(ws) => words.extend(ws.into_iter().map(|mut w| {
                    w.start_ms += base;
                    w.end_ms += base;
                    w.speaker = w.speaker.map(|s| format!("{base}:{s}")); // labels are per chunk
                    w
                })),
                Err(e) => {
                    eprintln!("chunk at {base} ms failed: {e:?}");
                    usage.failed_chunks += 1;
                }
            }
        }
        Ok(Transcript { words, usage })
    }
}
