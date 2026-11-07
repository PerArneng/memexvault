//! Edge: transcription, chosen by spec 027. Media decoding with `symphonia` and `opus-rs`, whisper.cpp with DTW word times and Silero VAD, and Gemini for text and speakers on the local clock. Callers run `worker::run` in a child process, because whisper.cpp can abort the process and the decoders read untrusted files.
// This crate reads and writes its work folder, waits between retries and talks to a provider, which is what it's for.
#![allow(clippy::disallowed_methods)]
pub mod decode;
pub mod gemini;
pub mod resample;
pub mod whisper;
pub mod worker;

pub use decode::{DecodeError, decode_to_file};
pub use gemini::GeminiApi;
pub use whisper::WhisperLocal;
