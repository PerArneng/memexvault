//! The API transcriber. Audio goes inline as 16-bit WAV, one chunk to a call. The key is passed in and never printed. It's built only when settings name it.
use memex_core::meeting::PCM_RATE;
use memex_core::{TranscribeError, Transcribed, Transcriber};
use memex_types::{TimedWord, TranscribeUsage};
use serde_json::{Value, json};
use std::time::Duration;

const DEFAULT_BASE: &str = "https://generativelanguage.googleapis.com";
const ATTEMPTS: u64 = 4;

pub struct GeminiApi {
    pub key: String,
    pub model: String,
    pub base_url: Option<String>,
    /// How long to wait before the second try, and then twice and three times that. Tests make it short.
    pub retry_after: Duration,
}

/// The audio as a 16-bit WAV file. Pure.
pub fn wav(pcm: &[f32]) -> Vec<u8> {
    let bytes = (pcm.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + bytes as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + bytes).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&PCM_RATE.to_le_bytes());
    out.extend_from_slice(&(PCM_RATE * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&bytes.to_le_bytes());
    for s in pcm {
        out.extend_from_slice(&((s.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
    }
    out
}

/// The words in a reply. None when it isn't what was asked for, which is worth another try. Pure.
pub fn parse_reply(reply: &Value) -> Option<(Vec<TimedWord>, TranscribeUsage)> {
    let meta = &reply["usageMetadata"];
    let usage = TranscribeUsage {
        input_tokens: meta["promptTokenCount"].as_u64().unwrap_or(0),
        output_tokens: meta["candidatesTokenCount"].as_u64().unwrap_or(0) + meta["thoughtsTokenCount"].as_u64().unwrap_or(0),
        calls: 1,
    };
    let text = reply["candidates"][0]["content"]["parts"][0]["text"].as_str()?;
    let items: Vec<Value> = serde_json::from_str(text).ok()?;
    let words = items
        .iter()
        .filter_map(|w| {
            Some(TimedWord {
                text: w["w"].as_str()?.trim().to_string(),
                start_ms: (w["s"].as_f64()? * 1000.0) as i64,
                end_ms: (w["e"].as_f64()? * 1000.0) as i64,
                speaker: w["spk"].as_str().map(str::to_string),
            })
        })
        .collect();
    Some((words, usage))
}

impl Transcriber for GeminiApi {
    fn model(&self) -> String {
        format!("gemini:{}", self.model)
    }

    fn transcribe(&self, pcm: &[f32], language: Option<&str>) -> Result<Transcribed, TranscribeError> {
        use base64::Engine;
        if pcm.is_empty() {
            return Ok(Transcribed::default());
        }
        let spoken = language.map_or("its spoken language".to_string(), |l| format!("its spoken language ({l})"));
        let prompt = format!(
            "Transcribe this recording verbatim, in {spoken}. Return every spoken word in order, one array item per word. \
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
        let base = self.base_url.as_deref().unwrap_or(DEFAULT_BASE).trim_end_matches('/');
        let url = format!("{base}/v1beta/models/{}:generateContent", self.model);
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(600)))
            .http_status_as_error(false)
            .build()
            .into();
        let mut spent = TranscribeUsage::default();
        let mut last = String::new();
        for attempt in 0..ATTEMPTS {
            if attempt > 0 {
                std::thread::sleep(self.retry_after * attempt as u32);
            }
            spent.calls += 1;
            let mut response = match agent.post(&url).header("x-goog-api-key", &self.key).send_json(&body) {
                Ok(r) => r,
                Err(e) => {
                    last = format!("can't reach the provider: {e}");
                    continue;
                }
            };
            let status = response.status().as_u16();
            let text = response
                .body_mut()
                .with_config()
                .limit(200 * 1024 * 1024)
                .read_to_string()
                .map_err(|e| TranscribeError::Unavailable(e.to_string()))?;
            if status != 200 {
                last = format!("the provider answered {status}: {}", text.chars().take(300).collect::<String>());
                if status == 429 || status >= 500 {
                    continue;
                }
                return Err(TranscribeError::Failed(last));
            }
            let reply: Value = serde_json::from_str(&text).map_err(|e| TranscribeError::Failed(format!("the provider's reply isn't JSON: {e}")))?;
            match parse_reply(&reply) {
                Some((words, usage)) => {
                    return Ok(Transcribed {
                        words,
                        usage: TranscribeUsage {
                            input_tokens: spent.input_tokens + usage.input_tokens,
                            output_tokens: spent.output_tokens + usage.output_tokens,
                            calls: spent.calls,
                        },
                    });
                }
                // The tokens were spent even though the reply can't be used.
                None => {
                    let meta = &reply["usageMetadata"];
                    spent.input_tokens += meta["promptTokenCount"].as_u64().unwrap_or(0);
                    spent.output_tokens += meta["candidatesTokenCount"].as_u64().unwrap_or(0) + meta["thoughtsTokenCount"].as_u64().unwrap_or(0);
                    last = "the provider's reply couldn't be parsed".to_string();
                }
            }
        }
        Err(TranscribeError::Unavailable(last))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_goes_as_16_bit_wav_and_is_clamped() {
        let w = wav(&[0.0, 1.0, -2.0]);
        assert_eq!(&w[..4], b"RIFF");
        assert_eq!(w.len(), 44 + 6);
        assert_eq!(i16::from_le_bytes([w[46], w[47]]), 32767);
        assert_eq!(i16::from_le_bytes([w[48], w[49]]), -32767);
    }

    #[test]
    fn a_reply_gives_words_speakers_and_usage() {
        let inner = json!([{"w": " hej ", "s": 1.5, "e": 2.0, "spk": "S1"}, {"w": "då", "s": 2.0, "e": 2.25}, {"nope": 1}]).to_string();
        let reply = json!({"candidates": [{"content": {"parts": [{"text": inner}]}}], "usageMetadata": {"promptTokenCount": 10, "candidatesTokenCount": 4, "thoughtsTokenCount": 1}});
        let (words, usage) = parse_reply(&reply).unwrap();
        assert_eq!(words.len(), 2);
        assert_eq!(
            (words[0].text.as_str(), words[0].start_ms, words[0].end_ms, words[0].speaker.as_deref()),
            ("hej", 1500, 2000, Some("S1"))
        );
        assert_eq!(words[1].speaker, None);
        assert_eq!((usage.input_tokens, usage.output_tokens), (10, 5));
    }

    #[test]
    fn a_reply_that_isnt_a_list_gives_nothing() {
        assert!(parse_reply(&json!({"candidates": [{"content": {"parts": [{"text": "sorry"}]}}]})).is_none());
        assert!(parse_reply(&json!({})).is_none());
    }
}
