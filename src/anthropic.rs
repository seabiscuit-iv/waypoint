//! Minimal Anthropic Messages API client (raw HTTP + SSE streaming).
//!
//! Rust has no official Anthropic SDK, so this speaks the wire protocol
//! directly: POST /v1/messages with `x-api-key` + `anthropic-version`
//! headers, streaming via Server-Sent Events.

use futures_util::StreamExt;
use serde::Serialize;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

pub const API_URL: &str = "https://api.anthropic.com/v1/messages";
pub const ANTHROPIC_VERSION: &str = "2023-06-01";

#[derive(Debug, Clone, Serialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

impl ChatMessage {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            content: content.into(),
        }
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: content.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct MessagesRequest {
    pub model: String,
    pub max_tokens: u32,
    pub system: String,
    pub messages: Vec<ChatMessage>,
    /// Effort level for models that support `output_config.effort`
    /// (None for models that reject it, e.g. Haiku 4.5).
    pub effort: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct CompletionResult {
    pub text: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub stop_reason: Option<String>,
}

/// Live token counter for a stream in flight.
///
/// The API bills for a cancelled or interrupted generation, so usage can't
/// only be collected from the success path — this is updated as the SSE
/// events arrive and stays readable after the stream future is dropped.
#[derive(Debug, Default)]
pub struct UsageMeter {
    input: AtomicI64,
    output: AtomicI64,
    text_chars: AtomicI64,
}

impl UsageMeter {
    fn set_input(&self, v: i64) {
        self.input.store(v, Ordering::Relaxed);
    }
    fn set_output(&self, v: i64) {
        self.output.store(v, Ordering::Relaxed);
    }
    fn set_chars(&self, v: i64) {
        self.text_chars.store(v, Ordering::Relaxed);
    }

    /// (input, output). Input is always the API's own figure. Output is the
    /// API's figure when the stream reached `message_delta`; otherwise it is
    /// estimated from the text received (~4 chars/token), since the tokens
    /// were generated and billed even though the final count never arrived.
    pub fn totals(&self) -> (i64, i64) {
        let input = self.input.load(Ordering::Relaxed);
        let output = self.output.load(Ordering::Relaxed);
        if output > 0 {
            (input, output)
        } else {
            (input, self.text_chars.load(Ordering::Relaxed) / 4)
        }
    }
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum ApiError {
    #[error("{0}")]
    Auth(String),
    #[error("{0}")]
    RateLimit(String),
    #[error("{0}")]
    Overloaded(String),
    #[error("{0}")]
    Network(String),
    #[error("{0}")]
    Api(String),
    #[error("Claude declined to generate this content (safety refusal).")]
    Refusal,
}

impl ApiError {
    pub fn kind(&self) -> &'static str {
        match self {
            ApiError::Auth(_) => "auth",
            ApiError::RateLimit(_) => "rate_limit",
            ApiError::Overloaded(_) => "overloaded",
            ApiError::Network(_) => "network",
            ApiError::Api(_) => "api",
            ApiError::Refusal => "refusal",
        }
    }
}

fn request_body(req: &MessagesRequest, stream: bool) -> Value {
    let mut body = json!({
        "model": req.model,
        "max_tokens": req.max_tokens,
        "messages": req.messages,
    });
    if !req.system.is_empty() {
        body["system"] = Value::String(req.system.clone());
    }
    if stream {
        body["stream"] = Value::Bool(true);
    }
    if let Some(effort) = &req.effort {
        body["output_config"] = json!({ "effort": effort });
    }
    body
}

fn describe_reqwest(e: &reqwest::Error) -> String {
    if e.is_connect() || e.is_timeout() {
        "Could not reach the Anthropic API. Check your internet connection.".to_string()
    } else if e.is_decode() || e.is_body() {
        "The connection was interrupted mid-response.".to_string()
    } else {
        format!("Network error: {e}")
    }
}

fn classify_error_type(err_type: &str, message: &str, status: u16) -> ApiError {
    let msg = |fallback: String| {
        if message.is_empty() {
            fallback
        } else {
            message.to_string()
        }
    };
    match err_type {
        "authentication_error" => ApiError::Auth(msg("The API key was rejected.".into())),
        "permission_error" => ApiError::Auth(msg("The API key lacks permission.".into())),
        "rate_limit_error" => ApiError::RateLimit(msg("Rate limited by the API.".into())),
        "overloaded_error" => {
            ApiError::Overloaded(msg("The API is temporarily overloaded.".into()))
        }
        _ => match status {
            401 | 403 => ApiError::Auth(msg("The API key was rejected.".into())),
            429 => ApiError::RateLimit(msg("Rate limited by the API.".into())),
            529 => ApiError::Overloaded(msg("The API is temporarily overloaded.".into())),
            _ => ApiError::Api(msg(format!("API error (HTTP {status})."))),
        },
    }
}

fn classify_http_error(status: u16, body: &str) -> ApiError {
    let parsed: Option<Value> = serde_json::from_str(body).ok();
    let (t, m) = parsed
        .as_ref()
        .map(|v| {
            (
                v["error"]["type"].as_str().unwrap_or("").to_string(),
                v["error"]["message"].as_str().unwrap_or("").to_string(),
            )
        })
        .unwrap_or_default();
    classify_error_type(&t, &m, status)
}

async fn send(
    http: &reqwest::Client,
    api_key: &str,
    body: &Value,
    timeout: Option<Duration>,
) -> Result<reqwest::Response, ApiError> {
    let mut builder = http
        .post(API_URL)
        .header("x-api-key", api_key)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header("content-type", "application/json")
        .json(body);
    if let Some(t) = timeout {
        builder = builder.timeout(t);
    }
    let resp = builder
        .send()
        .await
        .map_err(|e| ApiError::Network(describe_reqwest(&e)))?;
    let status = resp.status();
    if !status.is_success() {
        let text = resp.text().await.unwrap_or_default();
        return Err(classify_http_error(status.as_u16(), &text));
    }
    Ok(resp)
}

fn read_usage_input(usage: &Value) -> i64 {
    usage["input_tokens"].as_i64().unwrap_or(0)
        + usage["cache_creation_input_tokens"].as_i64().unwrap_or(0)
        + usage["cache_read_input_tokens"].as_i64().unwrap_or(0)
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// SSE events end with a blank line: `\n\n` normally, `\r\n\r\n` if the
/// transport is CRLF-delimited. Returns (content_end, delimiter_len).
fn find_event_boundary(buf: &[u8]) -> Option<(usize, usize)> {
    let lf = find_subslice(buf, b"\n\n").map(|p| (p, 2usize));
    let crlf = find_subslice(buf, b"\r\n\r\n").map(|p| (p, 4usize));
    match (lf, crlf) {
        (Some(a), Some(b)) => Some(if b.0 < a.0 { b } else { a }),
        (a, b) => a.or(b),
    }
}

fn handle_sse_block(
    block: &str,
    out: &mut CompletionResult,
    meter: &UsageMeter,
    on_text: &mut impl FnMut(&str),
) -> Result<(), ApiError> {
    for line in block.lines() {
        let line = line.trim_end_matches('\r');
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data.is_empty() {
            continue;
        }
        let Ok(v) = serde_json::from_str::<Value>(data) else {
            continue;
        };
        match v["type"].as_str().unwrap_or("") {
            "message_start" => {
                out.input_tokens = read_usage_input(&v["message"]["usage"]);
                meter.set_input(out.input_tokens);
            }
            "content_block_delta" => {
                if v["delta"]["type"].as_str() == Some("text_delta") {
                    if let Some(t) = v["delta"]["text"].as_str() {
                        out.text.push_str(t);
                        meter.set_chars(out.text.chars().count() as i64);
                        on_text(&out.text);
                    }
                }
            }
            "message_delta" => {
                if let Some(sr) = v["delta"]["stop_reason"].as_str() {
                    out.stop_reason = Some(sr.to_string());
                }
                if let Some(o) = v["usage"]["output_tokens"].as_i64() {
                    out.output_tokens = o;
                    meter.set_output(o);
                }
            }
            "error" => {
                let t = v["error"]["type"].as_str().unwrap_or("");
                let m = v["error"]["message"]
                    .as_str()
                    .unwrap_or("The stream reported an error.");
                return Err(classify_error_type(t, m, 0));
            }
            _ => {}
        }
    }
    Ok(())
}

/// Stream a message; `on_text` receives the full accumulated text each time
/// a new delta arrives.
pub async fn stream_message(
    http: &reqwest::Client,
    api_key: &str,
    req: &MessagesRequest,
    meter: &UsageMeter,
    mut on_text: impl FnMut(&str),
) -> Result<CompletionResult, ApiError> {
    let resp = send(http, api_key, &request_body(req, true), None).await?;
    let mut stream = resp.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    let mut out = CompletionResult::default();

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| ApiError::Network(describe_reqwest(&e)))?;
        buf.extend_from_slice(&chunk);
        // Splitting the byte buffer on the blank-line delimiter is UTF-8
        // safe: multi-byte sequences never contain 0x0A.
        while let Some((pos, delim)) = find_event_boundary(&buf) {
            let block: Vec<u8> = buf.drain(..pos + delim).collect();
            let text = String::from_utf8_lossy(&block[..pos]).into_owned();
            handle_sse_block(&text, &mut out, meter, &mut on_text)?;
        }
    }
    if !buf.is_empty() {
        // A final event without a trailing blank line.
        let text = String::from_utf8_lossy(&buf).into_owned();
        handle_sse_block(&text, &mut out, meter, &mut on_text)?;
    }

    if out.stop_reason.as_deref() == Some("refusal") {
        return Err(ApiError::Refusal);
    }
    Ok(out)
}

/// Non-streaming completion (used for concept-ledger extraction and the
/// API-key validation ping).
pub async fn complete_message(
    http: &reqwest::Client,
    api_key: &str,
    req: &MessagesRequest,
) -> Result<CompletionResult, ApiError> {
    let resp = send(
        http,
        api_key,
        &request_body(req, false),
        Some(Duration::from_secs(120)),
    )
    .await?;
    let v: Value = resp
        .json()
        .await
        .map_err(|e| ApiError::Network(describe_reqwest(&e)))?;

    let mut out = CompletionResult::default();
    if let Some(blocks) = v["content"].as_array() {
        for b in blocks {
            if b["type"].as_str() == Some("text") {
                if let Some(t) = b["text"].as_str() {
                    out.text.push_str(t);
                }
            }
        }
    }
    out.input_tokens = read_usage_input(&v["usage"]);
    out.output_tokens = v["usage"]["output_tokens"].as_i64().unwrap_or(0);
    out.stop_reason = v["stop_reason"].as_str().map(str::to_string);
    if out.stop_reason.as_deref() == Some("refusal") {
        return Err(ApiError::Refusal);
    }
    Ok(out)
}

/// USD list price per million tokens (input, output).
pub fn price_per_mtok(model: &str) -> (f64, f64) {
    if model.contains("haiku") {
        (1.0, 5.0)
    } else if model.contains("sonnet") {
        (3.0, 15.0)
    } else {
        // Opus 5 / Opus 4.8 tier (also the conservative default).
        (5.0, 25.0)
    }
}

pub fn cost_usd(model: &str, input_tokens: i64, output_tokens: i64) -> f64 {
    let (pi, po) = price_per_mtok(model);
    (input_tokens as f64 * pi + output_tokens as f64 * po) / 1_000_000.0
}
