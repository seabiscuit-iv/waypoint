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
    pub content: MessageContent,
}

/// A plain string, or content blocks when a message carries images.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum MessageContent {
    Text(String),
    Blocks(Vec<Value>),
}

impl ChatMessage {
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: "user".to_string(),
            content: MessageContent::Text(content.into()),
        }
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: MessageContent::Text(content.into()),
        }
    }
    pub fn user_blocks(blocks: Vec<Value>) -> Self {
        Self {
            role: "user".to_string(),
            content: MessageContent::Blocks(blocks),
        }
    }
}

/// Beta opt-in for the 1-hour cache TTL. The default 5-minute window expires
/// while the learner is reading a step, which is precisely when the cache
/// needs to survive.
pub const EXTENDED_CACHE_BETA: &str = "extended-cache-ttl-2025-04-11";

/// One block of the system prompt. Everything up to and including the block
/// marked `cache` forms the cached prefix, so cached blocks must come first
/// and must not change between calls.
#[derive(Debug, Clone)]
pub struct SystemBlock {
    pub text: String,
    pub cache: bool,
}

impl SystemBlock {
    pub fn stable(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            cache: true,
        }
    }
    pub fn volatile(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            cache: false,
        }
    }
}

#[derive(Debug, Clone)]
pub struct MessagesRequest {
    pub model: String,
    pub max_tokens: u32,
    pub system: Vec<SystemBlock>,
    pub messages: Vec<ChatMessage>,
    /// Effort level for models that support `output_config.effort`
    /// (None for models that reject it, e.g. Haiku 4.5).
    pub effort: Option<String>,
}

/// Token counts split by how they are billed.
#[derive(Debug, Clone, Copy, Default)]
pub struct Usage {
    /// Input billed at the normal rate (neither written to nor read from cache).
    pub input_tokens: i64,
    pub cache_write_tokens: i64,
    pub cache_read_tokens: i64,
    pub output_tokens: i64,
}

impl Usage {
    /// Every input token, however billed — what the UI reports.
    pub fn total_input(&self) -> i64 {
        self.input_tokens + self.cache_write_tokens + self.cache_read_tokens
    }
}

#[derive(Debug, Clone, Default)]
pub struct CompletionResult {
    pub text: String,
    pub usage: Usage,
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
    cache_write: AtomicI64,
    cache_read: AtomicI64,
    output: AtomicI64,
    text_chars: AtomicI64,
}

impl UsageMeter {
    fn set_input(&self, u: Usage) {
        self.input.store(u.input_tokens, Ordering::Relaxed);
        self.cache_write.store(u.cache_write_tokens, Ordering::Relaxed);
        self.cache_read.store(u.cache_read_tokens, Ordering::Relaxed);
    }
    fn set_output(&self, v: i64) {
        self.output.store(v, Ordering::Relaxed);
    }
    fn set_chars(&self, v: i64) {
        self.text_chars.store(v, Ordering::Relaxed);
    }

    /// Input figures are always the API's own. Output is the API's figure
    /// when the stream reached `message_delta`; otherwise it is estimated
    /// from the text received (~4 chars/token), since those tokens were
    /// generated and billed even though the final count never arrived.
    pub fn totals(&self) -> Usage {
        let output = self.output.load(Ordering::Relaxed);
        Usage {
            input_tokens: self.input.load(Ordering::Relaxed),
            cache_write_tokens: self.cache_write.load(Ordering::Relaxed),
            cache_read_tokens: self.cache_read.load(Ordering::Relaxed),
            output_tokens: if output > 0 {
                output
            } else {
                self.text_chars.load(Ordering::Relaxed) / 4
            },
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

fn uses_cache(req: &MessagesRequest) -> bool {
    req.system.iter().any(|b| b.cache && !b.text.is_empty())
}

fn request_body(req: &MessagesRequest, stream: bool) -> Value {
    let mut body = json!({
        "model": req.model,
        "max_tokens": req.max_tokens,
        "messages": req.messages,
    });

    let blocks: Vec<Value> = req
        .system
        .iter()
        .filter(|b| !b.text.is_empty())
        .map(|b| {
            let mut block = json!({ "type": "text", "text": b.text });
            if b.cache {
                block["cache_control"] = json!({ "type": "ephemeral", "ttl": "1h" });
            }
            block
        })
        .collect();
    if !blocks.is_empty() {
        body["system"] = Value::Array(blocks);
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
    cache: bool,
    timeout: Option<Duration>,
) -> Result<reqwest::Response, ApiError> {
    let mut builder = http
        .post(API_URL)
        .header("x-api-key", api_key)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header("content-type", "application/json")
        .json(body);
    if cache {
        builder = builder.header("anthropic-beta", EXTENDED_CACHE_BETA);
    }
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

fn read_usage_input(usage: &Value, out: &mut Usage) {
    out.input_tokens = usage["input_tokens"].as_i64().unwrap_or(0);
    out.cache_write_tokens = usage["cache_creation_input_tokens"].as_i64().unwrap_or(0);
    out.cache_read_tokens = usage["cache_read_input_tokens"].as_i64().unwrap_or(0);
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
                read_usage_input(&v["message"]["usage"], &mut out.usage);
                meter.set_input(out.usage);
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
                    out.usage.output_tokens = o;
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
    let resp = send(
        http,
        api_key,
        &request_body(req, true),
        uses_cache(req),
        None,
    )
    .await?;
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
        uses_cache(req),
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
    read_usage_input(&v["usage"], &mut out.usage);
    out.usage.output_tokens = v["usage"]["output_tokens"].as_i64().unwrap_or(0);
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

/// Cache writes on the 1-hour TTL bill at 2x the base input rate; cache
/// reads at 0.1x. Charging every input token at the base rate would overstate
/// a cached topic's spend by roughly an order of magnitude.
const CACHE_WRITE_MULTIPLIER: f64 = 2.0;
const CACHE_READ_MULTIPLIER: f64 = 0.1;

pub fn cost_usd(model: &str, usage: &Usage) -> f64 {
    let (pi, po) = price_per_mtok(model);
    let input = usage.input_tokens as f64 * pi
        + usage.cache_write_tokens as f64 * pi * CACHE_WRITE_MULTIPLIER
        + usage.cache_read_tokens as f64 * pi * CACHE_READ_MULTIPLIER;
    (input + usage.output_tokens as f64 * po) / 1_000_000.0
}
