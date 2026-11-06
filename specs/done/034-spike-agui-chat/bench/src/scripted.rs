//! Test models: a scripted `Llm`, and a fake OpenAI-compatible upstream that exercises ADK's real HTTP client.
use adk_core::{AdkError, Content, LlmRequest, LlmResponse, LlmResponseStream, Part, Result};
use async_trait::async_trait;
use axum::{body::Body, extract::Path, http::{header, StatusCode}, response::{IntoResponse, Response}};
use std::time::Duration;

pub struct ScriptedLlm { pub mode: String }

fn text_chunk(t: &str) -> LlmResponse {
    LlmResponse { content: Some(Content { role: "model".into(), parts: vec![Part::Text { text: t.into() }] }), partial: true, ..Default::default() }
}

fn call_chunk(name: &str, args: serde_json::Value) -> LlmResponse {
    LlmResponse { content: Some(Content { role: "model".into(), parts: vec![Part::FunctionCall { name: name.into(), args, id: None, thought_signature: None }] }), partial: true, ..Default::default() }
}

fn end_chunk() -> LlmResponse {
    LlmResponse { partial: false, turn_complete: true, finish_reason: Some(adk_core::FinishReason::Stop), ..Default::default() }
}

#[async_trait]
impl adk_core::Llm for ScriptedLlm {
    fn name(&self) -> &str { "scripted" }

    async fn generate_content(&self, req: LlmRequest, _stream: bool) -> Result<LlmResponseStream> {
        let after_tool = req.contents.iter().flat_map(|c| &c.parts).any(|p| matches!(p, Part::FunctionResponse { .. }));
        let mode = self.mode.clone();
        Ok(Box::pin(async_stream::stream! {
            match mode.as_str() {
                "tool" if !after_tool => {
                    yield Ok(text_chunk("Let me "));
                    yield Ok(text_chunk("check."));
                    yield Ok(call_chunk("lookup_fact", serde_json::json!({"topic": "bridge"})));
                    yield Ok(end_chunk());
                }
                "slowtool" if !after_tool => {
                    yield Ok(call_chunk("slow_lookup", serde_json::json!({"topic": "bridge"})));
                    yield Ok(end_chunk());
                }
                "slow" => {
                    for i in 0..2000 { yield Ok(text_chunk(&format!("w{i} "))); tokio::time::sleep(Duration::from_millis(50)).await; }
                    yield Ok(end_chunk());
                }
                "fail" => {
                    for t in ["one ", "two ", "three "] { yield Ok(text_chunk(t)); }
                    yield Err(AdkError::model("scripted failure"));
                }
                "errchunk" => {
                    yield Ok(text_chunk("one "));
                    yield Ok(LlmResponse { error_code: Some("SAFETY".into()), error_message: Some("blocked".into()), partial: false, turn_complete: true, ..Default::default() });
                }
                _ => {
                    // Splits a word, an accent and an emoji across chunks. Only ever at char boundaries, as Rust strings must.
                    for t in ["The Zor", "blat Br", "idge ", "is 412 m", "etres lo", "ng, sä", "ger ", "Å", "sa 🌉", "."] { yield Ok(text_chunk(t)); }
                    yield Ok(end_chunk());
                }
            }
        }))
    }
}

/// When the fake upstream last saw its caller hang up, in wall-clock milliseconds.
pub static UPSTREAM_CLOSED_MS: std::sync::Mutex<Option<f64>> = std::sync::Mutex::new(None);

struct UpstreamClosed;
impl Drop for UpstreamClosed {
    fn drop(&mut self) { *UPSTREAM_CLOSED_MS.lock().unwrap() = Some(crate::now_ms()); }
}

fn sse_chunk(delta: &str) -> String {
    format!("data: {}\n\n", serde_json::json!({"id":"x","object":"chat.completion.chunk","choices":[{"index":0,"delta":{"content":delta},"finish_reason":null}]}))
}

/// `POST /fake/{behavior}/v1/chat/completions`. Answers at once, so anything measured through it is our cost.
pub async fn fake_upstream(Path(behavior): Path<String>) -> Response {
    if behavior == "500" { return (StatusCode::INTERNAL_SERVER_ERROR, "upstream on fire").into_response(); }
    if behavior == "slow" {
        // Never finishes by itself. What ends it is the caller closing the connection, seen here as this stream dropping.
        let body = async_stream::stream! {
            let _closed = UpstreamClosed;
            for i in 0..100_000 {
                yield Ok::<_, std::io::Error>(sse_chunk(&format!("tok{i} ")));
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        };
        return ([(header::CONTENT_TYPE, "text/event-stream")], Body::from_stream(body)).into_response();
    }
    let body = async_stream::stream! {
        for i in 0..8 {
            if behavior == "cut" && i == 3 { yield Err(std::io::Error::other("connection cut")); return; }
            yield Ok::<_, std::io::Error>(sse_chunk(&format!("tok{i} ")));
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        yield Ok(format!("data: {}\n\ndata: [DONE]\n\n", serde_json::json!({"id":"x","object":"chat.completion.chunk","choices":[{"index":0,"delta":{},"finish_reason":"stop"}]})));
    };
    ([(header::CONTENT_TYPE, "text/event-stream")], Body::from_stream(body)).into_response()
}
