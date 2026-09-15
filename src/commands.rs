//! Tauri commands + background generation tasks.
//!
//! Generation is asynchronous: a command registers a job, returns a `gen_id`
//! immediately, and the spawned task streams progress to the webview via
//! events (`gen:start`, `gen:delta`, `gen:done`, `note:done`, `gen:error`,
//! `ledger:updated`). Every mutation is persisted as it happens (autosave).

use std::path::PathBuf;
use std::sync::MutexGuard;
use std::time::{Duration, Instant};

use chrono::Utc;
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use tokio::sync::oneshot;
use uuid::Uuid;

use crate::anthropic::{self, MessagesRequest};
use crate::auth;
use crate::db;
use crate::documents;
use crate::error::CmdError;
use crate::markdown;
use crate::models::*;
use crate::prompts;
use crate::svg;
use crate::AppState;

const ALLOWED_MODELS: [&str; 3] = ["claude-opus-5", "claude-sonnet-5", "claude-haiku-4-5"];
const ALLOWED_STEP_SIZES: [&str; 3] = ["brief", "standard", "deep"];
const ALLOWED_THEMES: [&str; 3] = ["system", "light", "dark"];
const ALLOWED_STATUSES: [&str; 3] = ["learning", "revisit", "done"];

fn now_iso() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}

fn new_id() -> String {
    Uuid::new_v4().to_string()
}

fn lock_db<'a>(state: &'a AppState) -> MutexGuard<'a, rusqlite::Connection> {
    state.db.lock().expect("db mutex poisoned")
}

fn clean_opt(s: Option<String>) -> Option<String> {
    s.map(|v| v.trim().to_string()).filter(|v| !v.is_empty())
}

// ------------------------------------------------------------- payloads --

#[derive(Debug, Clone, Serialize)]
pub struct AuthStatus {
    pub configured: bool,
    pub masked_key: Option<String>,
    /// Where the key is stored on this platform, for display.
    pub store_name: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct KeyTestResult {
    pub ok: bool,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct GenStarted {
    pub gen_id: String,
    pub step_id: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct NoteGenStarted {
    pub gen_id: String,
    pub note: SideNote,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImportedFile {
    pub name: String,
    pub content: String,
    /// Set when this particular file couldn't be read; the rest of a batch
    /// still comes through.
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct GenStartEvent {
    gen_id: String,
    topic_id: String,
    kind: String,
    step_id: Option<String>,
    note_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct GenDeltaEvent {
    gen_id: String,
    raw: String,
    html: String,
}

#[derive(Debug, Clone, Serialize)]
struct GenErrorEvent {
    gen_id: String,
    topic_id: String,
    kind: String,
    message: String,
    context_kind: String,
    step_id: Option<String>,
    note_id: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
struct SpineDoneEvent {
    gen_id: String,
    topic_id: String,
    kind: String,
    step: SpineStep,
    usage: UsageTotals,
    /// A diagram review follows; its result arrives as `step:updated`.
    reviewing: bool,
}

#[derive(Debug, Clone, Serialize)]
struct NoteDoneEvent {
    gen_id: String,
    topic_id: String,
    note_id: String,
    message: SideNoteMessage,
    usage: UsageTotals,
    /// A diagram review follows; its result arrives as `note:updated`.
    reviewing: bool,
}

#[derive(Debug, Clone, Serialize)]
struct LedgerEvent {
    topic_id: String,
    entries: Vec<ConceptEntry>,
    usage: UsageTotals,
}

// ----------------------------------------------------------------- auth --

fn mask_key(k: &str) -> String {
    let chars: Vec<char> = k.chars().collect();
    if chars.len() <= 14 {
        return "\u{2022}\u{2022}\u{2022}".to_string();
    }
    let head: String = chars[..10].iter().collect();
    let tail: String = chars[chars.len() - 4..].iter().collect();
    format!("{head}\u{2026}{tail}")
}

#[tauri::command]
pub fn get_auth_status() -> AuthStatus {
    let store_name = auth::store_name().to_string();
    match auth::get_key() {
        Some(k) => AuthStatus {
            configured: true,
            masked_key: Some(mask_key(&k)),
            store_name,
        },
        None => AuthStatus {
            configured: false,
            masked_key: None,
            store_name,
        },
    }
}

#[tauri::command]
pub fn set_api_key(key: String) -> Result<AuthStatus, CmdError> {
    let key = key.trim().to_string();
    if key.is_empty() {
        return Err(CmdError::new("invalid", "The API key is empty."));
    }
    auth::set_key(&key)?;
    Ok(get_auth_status())
}

#[tauri::command]
pub fn clear_api_key() -> Result<AuthStatus, CmdError> {
    auth::clear_key()?;
    Ok(get_auth_status())
}

#[tauri::command]
pub async fn test_api_key(
    state: State<'_, AppState>,
    key: Option<String>,
) -> Result<KeyTestResult, CmdError> {
    let key = match clean_opt(key) {
        Some(k) => k,
        None => auth::require_key()?,
    };
    let req = prompts::test_key_request();
    match anthropic::complete_message(&state.http, &key, &req).await {
        Ok(_) => Ok(KeyTestResult {
            ok: true,
            message: "Key verified. Connected to the Anthropic API.".to_string(),
        }),
        Err(e) => Ok(KeyTestResult {
            ok: false,
            message: e.to_string(),
        }),
    }
}

// ------------------------------------------------------------- settings --

#[tauri::command]
pub fn get_settings(state: State<'_, AppState>) -> Result<Settings, CmdError> {
    let conn = lock_db(&state);
    Ok(db::get_settings(&conn)?)
}

#[tauri::command]
pub fn update_settings(
    state: State<'_, AppState>,
    settings: Settings,
) -> Result<Settings, CmdError> {
    if !ALLOWED_MODELS.contains(&settings.model.as_str()) {
        return Err(CmdError::new("invalid", "Unknown model."));
    }
    if !ALLOWED_STEP_SIZES.contains(&settings.step_size.as_str()) {
        return Err(CmdError::new("invalid", "Unknown step size."));
    }
    if !ALLOWED_THEMES.contains(&settings.theme.as_str()) {
        return Err(CmdError::new("invalid", "Unknown theme."));
    }
    let conn = lock_db(&state);
    db::set_settings(&conn, &settings)?;
    Ok(settings)
}

// --------------------------------------------------------------- topics --

#[tauri::command]
pub fn list_topics(state: State<'_, AppState>) -> Result<Vec<TopicSummary>, CmdError> {
    let conn = lock_db(&state);
    Ok(db::list_topics(&conn)?)
}

#[tauri::command]
pub fn create_topic(
    state: State<'_, AppState>,
    title: String,
    seed_context: Option<String>,
    prior_knowledge: Option<String>,
) -> Result<TopicSummary, CmdError> {
    // §5.1: topic creation is gated on a configured key, not just hidden
    // behind the onboarding screen.
    auth::require_key()?;
    let title = title.trim().to_string();
    if title.is_empty() {
        return Err(CmdError::new("invalid", "The topic needs a title."));
    }
    let seed = clean_opt(seed_context);
    let prior = clean_opt(prior_knowledge);
    let id = new_id();
    let conn = lock_db(&state);
    db::create_topic(&conn, &id, &title, seed.as_deref(), prior.as_deref(), &now_iso())?;
    Ok(db::get_topic_summary(&conn, &id)?)
}

#[tauri::command]
pub fn get_topic(state: State<'_, AppState>, topic_id: String) -> Result<TopicDetail, CmdError> {
    let conn = lock_db(&state);
    Ok(db::get_topic_detail(&conn, &topic_id)?)
}

#[tauri::command]
pub fn rename_topic(
    state: State<'_, AppState>,
    topic_id: String,
    title: String,
) -> Result<(), CmdError> {
    let title = title.trim().to_string();
    if title.is_empty() {
        return Err(CmdError::new("invalid", "The topic needs a title."));
    }
    let conn = lock_db(&state);
    db::rename_topic(&conn, &topic_id, &title, &now_iso())?;
    Ok(())
}

#[tauri::command]
pub fn delete_topic(state: State<'_, AppState>, topic_id: String) -> Result<(), CmdError> {
    let conn = lock_db(&state);
    db::delete_topic(&conn, &topic_id)?;
    Ok(())
}

#[tauri::command]
pub fn duplicate_topic(
    state: State<'_, AppState>,
    topic_id: String,
) -> Result<TopicSummary, CmdError> {
    let mut conn = lock_db(&state);
    let new_id = db::duplicate_topic(&mut conn, &topic_id, &now_iso())?;
    Ok(db::get_topic_summary(&conn, &new_id)?)
}

#[tauri::command]
pub fn set_topic_status(
    state: State<'_, AppState>,
    topic_id: String,
    status: String,
) -> Result<(), CmdError> {
    if !ALLOWED_STATUSES.contains(&status.as_str()) {
        return Err(CmdError::new("invalid", "Unknown status."));
    }
    let conn = lock_db(&state);
    db::set_topic_status(&conn, &topic_id, &status, &now_iso())?;
    Ok(())
}

#[tauri::command]
pub fn reorder_topics(state: State<'_, AppState>, ids: Vec<String>) -> Result<(), CmdError> {
    let conn = lock_db(&state);
    db::reorder_topics(&conn, &ids)?;
    Ok(())
}

// ------------------------------------------------------ generation core --

struct GenFailure {
    kind: String,
    message: String,
}

/// Streams a request, forwarding throttled deltas to the webview.
/// Cancellation arrives via the oneshot receiver.
async fn run_generation(
    app: &AppHandle,
    gen_id: &str,
    request: &MessagesRequest,
    cancel: oneshot::Receiver<()>,
    meter: &anthropic::UsageMeter,
) -> Result<anthropic::CompletionResult, GenFailure> {
    let Some(key) = auth::get_key() else {
        return Err(GenFailure {
            kind: "auth".to_string(),
            message: "No API key is configured. Add one in Settings.".to_string(),
        });
    };
    let http = {
        let state = app.state::<AppState>();
        state.http.clone()
    };

    let app_for_delta = app.clone();
    let gen_id_owned = gen_id.to_string();
    let mut last_emit = Instant::now()
        .checked_sub(Duration::from_millis(500))
        .unwrap_or_else(Instant::now);

    let stream_fut = anthropic::stream_message(&http, &key, request, meter, move |full: &str| {
        // ~30fps. The webview coalesces to one DOM write per frame anyway, so
        // a faster cadence only doubles IPC traffic and re-render cost for
        // frames nobody sees.
        if last_emit.elapsed() >= Duration::from_millis(33) {
            last_emit = Instant::now();
            let _ = app_for_delta.emit(
                "gen:delta",
                GenDeltaEvent {
                    gen_id: gen_id_owned.clone(),
                    raw: full.to_string(),
                    html: markdown::render_stream(full),
                },
            );
        }
    });

    tokio::select! {
        res = stream_fut => res.map_err(|e| GenFailure {
            kind: e.kind().to_string(),
            message: e.to_string(),
        }),
        _ = cancel => Err(GenFailure {
            kind: "cancelled".to_string(),
            message: "Generation stopped.".to_string(),
        }),
    }
}

fn register_gen(app: &AppHandle, gen_id: &str) -> oneshot::Receiver<()> {
    let (tx, rx) = oneshot::channel();
    let state = app.state::<AppState>();
    state
        .gens
        .lock()
        .expect("gens mutex poisoned")
        .insert(gen_id.to_string(), tx);
    rx
}

fn unregister_gen(app: &AppHandle, gen_id: &str) {
    let state = app.state::<AppState>();
    state
        .gens
        .lock()
        .expect("gens mutex poisoned")
        .remove(gen_id);
}

#[tauri::command]
pub fn cancel_generation(state: State<'_, AppState>, gen_id: String) -> Result<bool, CmdError> {
    let sender = state
        .gens
        .lock()
        .expect("gens mutex poisoned")
        .remove(&gen_id);
    match sender {
        Some(tx) => {
            let _ = tx.send(());
            Ok(true)
        }
        None => Ok(false),
    }
}

/// Records tokens for a generation that ended without a clean result.
///
/// A cancelled or interrupted stream is still billed: the input was charged
/// when the request was accepted, and the output produced before the break
/// was charged too. Dropping it would make the running cost (§5.9) drift
/// steadily below the real bill for anyone who stops or retries often.
fn record_partial_usage(
    app: &AppHandle,
    topic_id: &str,
    request: &MessagesRequest,
    meter: &anthropic::UsageMeter,
) {
    let usage = meter.totals();
    if usage.total_input() == 0 && usage.output_tokens == 0 {
        return;
    }
    let cost = anthropic::cost_usd(&request.model, &usage);
    {
        let state = app.state::<AppState>();
        let conn = lock_db(&state);
        let _ = db::add_usage(
            &conn,
            topic_id,
            usage.total_input(),
            usage.output_tokens,
            cost,
            &now_iso(),
        );
    }
    // Carries the refreshed usage totals to the UI.
    emit_ledger_state(app, topic_id);
}

fn emit_ledger_state(app: &AppHandle, topic_id: &str) {
    let state = app.state::<AppState>();
    let conn = lock_db(&state);
    let entries = db::get_ledger(&conn, topic_id).unwrap_or_default();
    let usage = db::get_usage(&conn, topic_id).unwrap_or_default();
    let _ = app.emit(
        "ledger:updated",
        LedgerEvent {
            topic_id: topic_id.to_string(),
            entries,
            usage,
        },
    );
}

/// Fire-and-forget concept extraction (cheap Haiku call). Failures are
/// silent — the ledger is an optimization, not a correctness requirement.
fn spawn_ledger_extraction(
    app: AppHandle,
    topic_id: String,
    source_kind: String,
    source_id: String,
    excerpt: String,
) {
    tauri::async_runtime::spawn(async move {
        let Some(key) = auth::get_key() else { return };
        let (existing, http) = {
            let state = app.state::<AppState>();
            let conn = lock_db(&state);
            let Ok(existing) = db::ledger_labels(&conn, &topic_id) else {
                return;
            };
            (existing, state.http.clone())
        };
        let request = prompts::build_ledger_request(&excerpt, &existing);
        let Ok(res) = anthropic::complete_message(&http, &key, &request).await else {
            return;
        };
        let labels = prompts::parse_ledger_labels(&res.text);
        {
            let state = app.state::<AppState>();
            let conn = lock_db(&state);
            let now = now_iso();
            if !labels.is_empty() {
                let _ = db::insert_ledger_entries(
                    &conn, &topic_id, &labels, &source_kind, &source_id, &now,
                );
            }
            let cost = anthropic::cost_usd(&request.model, &res.usage);
            let _ = db::add_usage(
                &conn,
                &topic_id,
                res.usage.total_input(),
                res.usage.output_tokens,
                cost,
                &now,
            );
        }
        emit_ledger_state(&app, &topic_id);
    });
}

#[derive(Debug, Clone, Serialize)]
struct StepUpdatedEvent {
    topic_id: String,
    step: SpineStep,
}

#[derive(Debug, Clone, Serialize)]
struct NoteMessageUpdatedEvent {
    topic_id: String,
    note_id: String,
    message: SideNoteMessage,
}

/// Where reviewed diagrams live: a spine step or a side-note message.
enum ReviewTarget {
    Step { step_id: String },
    NoteMessage { note_id: String, message_id: String },
}

/// Whether finished content's diagrams go through `spawn_diagram_review`:
/// the experimental diagrams setting is on and there's something to check.
/// The UI holds them back until that review reports in with `step:updated`
/// or `note:updated`.
fn needs_diagram_review(app: &AppHandle, content: &str) -> bool {
    let enabled = {
        let state = app.state::<AppState>();
        let conn = lock_db(&state);
        db::get_settings(&conn).is_ok_and(|s| s.diagrams)
    };
    enabled && !reviewable_diagrams(content).is_empty()
}

/// Diagrams that can be corrected in place: their contents range and source.
/// `needs_diagram_review` and `spawn_diagram_review` must agree on this, or
/// the UI would wait for a review that never starts.
fn reviewable_diagrams(content: &str) -> Vec<(std::ops::Range<usize>, String)> {
    markdown::svg_blocks(content)
        .into_iter()
        .filter_map(|b| Some((b.contents?, b.source)))
        .collect()
}

/// Fire-and-forget diagram check: each svg block in finished content is
/// rendered to an image and sent back to the model, and any corrected SVG is
/// saved in place. Failures leave the content exactly as generated.
fn spawn_diagram_review(app: AppHandle, topic_id: String, target: ReviewTarget, content: String, model: String) {
    let blocks = reviewable_diagrams(&content);
    if blocks.is_empty() {
        return;
    }
    tauri::async_runtime::spawn(async move {
        if let Some(key) = auth::get_key() {
            let revised = review_diagrams(&app, &topic_id, &key, &content, &model, blocks).await;
            if revised != content {
                let state = app.state::<AppState>();
                let conn = lock_db(&state);
                let _ = match &target {
                    ReviewTarget::Step { step_id } => {
                        db::revise_step_content(&conn, step_id, &content, &revised)
                    }
                    ReviewTarget::NoteMessage { message_id, .. } => {
                        db::revise_note_message_content(&conn, message_id, &content, &revised)
                    }
                };
            }
            emit_ledger_state(&app, &topic_id);
        }

        // Always report back, changed or not: the UI is holding these
        // diagrams until this arrives.
        let state = app.state::<AppState>();
        match target {
            ReviewTarget::Step { step_id } => {
                let step = db::get_step(&lock_db(&state), &step_id).ok();
                if let Some(step) = step {
                    let _ = app.emit("step:updated", StepUpdatedEvent { topic_id, step });
                }
            }
            ReviewTarget::NoteMessage { note_id, message_id } => {
                let message = db::get_note_message(&lock_db(&state), &message_id).ok();
                if let Some(message) = message {
                    let _ = app.emit(
                        "note:updated",
                        NoteMessageUpdatedEvent { topic_id, note_id, message },
                    );
                }
            }
        }
    });
}

/// The step content with every diagram the reviewer corrected swapped in.
async fn review_diagrams(
    app: &AppHandle,
    topic_id: &str,
    key: &str,
    content: &str,
    model: &str,
    blocks: Vec<(std::ops::Range<usize>, String)>,
) -> String {
    let http = app.state::<AppState>().http.clone();

    // Last block first, so earlier byte ranges stay valid as later blocks
    // change length.
    let mut revised = content.to_string();
    for (range, source) in blocks.into_iter().rev() {
        let render_src = source.clone();
        let Ok(Some(png)) =
            tauri::async_runtime::spawn_blocking(move || svg::preview_png(&render_src)).await
        else {
            continue;
        };
        let request = prompts::build_diagram_review_request(model, content, &source, &png);
        let Ok(res) = anthropic::complete_message(&http, key, &request).await else {
            continue;
        };
        {
            let state = app.state::<AppState>();
            let conn = lock_db(&state);
            let cost = anthropic::cost_usd(&request.model, &res.usage);
            let _ = db::add_usage(
                &conn,
                topic_id,
                res.usage.total_input(),
                res.usage.output_tokens,
                cost,
                &now_iso(),
            );
        }
        if let Some(fixed) = prompts::parse_diagram_review(&res.text, &source) {
            revised.replace_range(range, &fixed);
        }
    }
    revised
}

#[derive(Clone, Copy, PartialEq)]
enum SpineMode {
    Append,
    Replace,
}

fn spawn_spine_generation(
    app: AppHandle,
    gen_id: String,
    topic_id: String,
    step_id: String,
    prompt_to_store: Option<String>,
    request: MessagesRequest,
    mode: SpineMode,
) {
    let cancel_rx = register_gen(&app, &gen_id);
    tauri::async_runtime::spawn(async move {
        let kind = match mode {
            SpineMode::Append => "spine",
            SpineMode::Replace => "regen",
        }
        .to_string();

        let _ = app.emit(
            "gen:start",
            GenStartEvent {
                gen_id: gen_id.clone(),
                topic_id: topic_id.clone(),
                kind: kind.clone(),
                step_id: Some(step_id.clone()),
                note_id: None,
            },
        );

        let meter = anthropic::UsageMeter::default();
        let outcome = run_generation(&app, &gen_id, &request, cancel_rx, &meter).await;
        unregister_gen(&app, &gen_id);

        match outcome {
            Ok(res) => {
                let persisted = {
                    let state = app.state::<AppState>();
                    let conn = lock_db(&state);
                    let now = now_iso();
                    let write = (|| -> Result<(SpineStep, UsageTotals), rusqlite::Error> {
                        match mode {
                            SpineMode::Append => {
                                let position = db::next_step_position(&conn, &topic_id)?;
                                db::insert_step(
                                    &conn,
                                    &step_id,
                                    &topic_id,
                                    position,
                                    prompt_to_store.as_deref(),
                                    &res.text,
                                    &now,
                                )?;
                            }
                            SpineMode::Replace => {
                                db::replace_step(
                                    &conn,
                                    &step_id,
                                    prompt_to_store.as_deref(),
                                    &res.text,
                                    &now,
                                )?;
                                db::delete_ledger_for_source(&conn, &step_id)?;
                            }
                        }
                        let cost = anthropic::cost_usd(&request.model, &res.usage);
                        db::add_usage(
                            &conn,
                            &topic_id,
                            res.usage.total_input(),
                            res.usage.output_tokens,
                            cost,
                            &now,
                        )?;
                        let step = db::get_step(&conn, &step_id)?;
                        let usage = db::get_usage(&conn, &topic_id)?;
                        Ok((step, usage))
                    })();
                    write
                };
                match persisted {
                    Ok((step, usage)) => {
                        let reviewing = needs_diagram_review(&app, &step.content);
                        let _ = app.emit(
                            "gen:done",
                            SpineDoneEvent {
                                gen_id,
                                topic_id: topic_id.clone(),
                                kind,
                                step: step.clone(),
                                usage,
                                reviewing,
                            },
                        );
                        if mode == SpineMode::Replace {
                            emit_ledger_state(&app, &topic_id);
                        }
                        if reviewing {
                            spawn_diagram_review(
                                app.clone(),
                                topic_id.clone(),
                                ReviewTarget::Step { step_id: step.id.clone() },
                                step.content.clone(),
                                request.model.clone(),
                            );
                        }
                        spawn_ledger_extraction(
                            app,
                            topic_id,
                            "spine".to_string(),
                            step.id.clone(),
                            step.content,
                        );
                    }
                    Err(e) => {
                        let _ = app.emit(
                            "gen:error",
                            GenErrorEvent {
                                gen_id,
                                topic_id,
                                kind: "db".to_string(),
                                message: e.to_string(),
                                context_kind: kind,
                                step_id: Some(step_id),
                                note_id: None,
                            },
                        );
                    }
                }
            }
            Err(err) => {
                let _ = app.emit(
                    "gen:error",
                    GenErrorEvent {
                        gen_id,
                        topic_id: topic_id.clone(),
                        kind: err.kind,
                        message: err.message,
                        context_kind: kind,
                        step_id: Some(step_id),
                        note_id: None,
                    },
                );
                record_partial_usage(&app, &topic_id, &request, &meter);
            }
        }
    });
}

fn spawn_note_generation(
    app: AppHandle,
    gen_id: String,
    topic_id: String,
    note_id: String,
    request: MessagesRequest,
) {
    let cancel_rx = register_gen(&app, &gen_id);
    tauri::async_runtime::spawn(async move {
        let _ = app.emit(
            "gen:start",
            GenStartEvent {
                gen_id: gen_id.clone(),
                topic_id: topic_id.clone(),
                kind: "side_note".to_string(),
                step_id: None,
                note_id: Some(note_id.clone()),
            },
        );

        let meter = anthropic::UsageMeter::default();
        let outcome = run_generation(&app, &gen_id, &request, cancel_rx, &meter).await;
        unregister_gen(&app, &gen_id);

        match outcome {
            Ok(res) => {
                let persisted = {
                    let state = app.state::<AppState>();
                    let conn = lock_db(&state);
                    let now = now_iso();
                    (|| -> Result<(SideNoteMessage, UsageTotals), rusqlite::Error> {
                        let msg_id = new_id();
                        db::insert_note_message(
                            &conn, &msg_id, &note_id, "assistant", &res.text, &now,
                        )?;
                        let cost = anthropic::cost_usd(&request.model, &res.usage);
                        db::add_usage(
                            &conn,
                            &topic_id,
                            res.usage.total_input(),
                            res.usage.output_tokens,
                            cost,
                            &now,
                        )?;
                        let message = db::get_note_message(&conn, &msg_id)?;
                        let usage = db::get_usage(&conn, &topic_id)?;
                        Ok((message, usage))
                    })()
                };
                match persisted {
                    Ok((message, usage)) => {
                        let reviewing = needs_diagram_review(&app, &message.content);
                        let _ = app.emit(
                            "note:done",
                            NoteDoneEvent {
                                gen_id,
                                topic_id: topic_id.clone(),
                                note_id: note_id.clone(),
                                message: message.clone(),
                                usage,
                                reviewing,
                            },
                        );
                        if reviewing {
                            spawn_diagram_review(
                                app,
                                topic_id,
                                ReviewTarget::NoteMessage { note_id, message_id: message.id },
                                message.content,
                                request.model.clone(),
                            );
                        }
                    }
                    Err(e) => {
                        let _ = app.emit(
                            "gen:error",
                            GenErrorEvent {
                                gen_id,
                                topic_id,
                                kind: "db".to_string(),
                                message: e.to_string(),
                                context_kind: "side_note".to_string(),
                                step_id: None,
                                note_id: Some(note_id),
                            },
                        );
                    }
                }
            }
            Err(err) => {
                let _ = app.emit(
                    "gen:error",
                    GenErrorEvent {
                        gen_id,
                        topic_id: topic_id.clone(),
                        kind: err.kind,
                        message: err.message,
                        context_kind: "side_note".to_string(),
                        step_id: None,
                        note_id: Some(note_id),
                    },
                );
                record_partial_usage(&app, &topic_id, &request, &meter);
            }
        }
    });
}

// ---------------------------------------------------------------- spine --

#[tauri::command]
pub fn advance_spine(
    app: AppHandle,
    topic_id: String,
    steering: Option<String>,
) -> Result<GenStarted, CmdError> {
    auth::require_key()?;
    let steering = clean_opt(steering);
    let request = {
        let state = app.state::<AppState>();
        let conn = lock_db(&state);
        let settings = db::get_settings(&conn)?;
        let topic = db::get_topic_row(&conn, &topic_id)?;
        let steps = db::get_steps(&conn, &topic_id)?;
        let ledger = db::ledger_labels(&conn, &topic_id)?;
        prompts::build_spine_request(
            &settings,
            &topic.title,
            topic.seed_context.as_deref(),
            topic.prior_knowledge.as_deref(),
            &steps,
            &ledger,
            steering.as_deref(),
        )
    };
    let gen_id = new_id();
    let step_id = new_id();
    spawn_spine_generation(
        app,
        gen_id.clone(),
        topic_id,
        step_id.clone(),
        steering,
        request,
        SpineMode::Append,
    );
    Ok(GenStarted { gen_id, step_id })
}

#[tauri::command]
pub fn regenerate_step(
    app: AppHandle,
    step_id: String,
    steering: Option<String>,
) -> Result<GenStarted, CmdError> {
    auth::require_key()?;
    let steering = clean_opt(steering);
    let (request, topic_id, prompt_to_store) = {
        let state = app.state::<AppState>();
        let conn = lock_db(&state);
        let step = db::get_step(&conn, &step_id)?;
        let topic = db::get_topic_row(&conn, &step.topic_id)?;
        let settings = db::get_settings(&conn)?;
        let all_steps = db::get_steps(&conn, &step.topic_id)?;
        let prior: Vec<SpineStep> = all_steps
            .into_iter()
            .filter(|s| s.position < step.position)
            .collect();
        let ledger = db::ledger_labels_excluding(&conn, &step.topic_id, &step_id)?;
        let prompt_to_store = steering.clone().or_else(|| step.prompt.clone());
        let request = prompts::build_regen_request(
            &settings,
            &topic.title,
            topic.seed_context.as_deref(),
            topic.prior_knowledge.as_deref(),
            &prior,
            &ledger,
            steering.as_deref(),
            &step.content,
        );
        (request, step.topic_id.clone(), prompt_to_store)
    };
    let gen_id = new_id();
    spawn_spine_generation(
        app,
        gen_id.clone(),
        topic_id,
        step_id.clone(),
        prompt_to_store,
        request,
        SpineMode::Replace,
    );
    Ok(GenStarted { gen_id, step_id })
}

#[tauri::command]
pub fn edit_step(app: AppHandle, step_id: String, content: String) -> Result<SpineStep, CmdError> {
    let content = content.trim().to_string();
    if content.is_empty() {
        return Err(CmdError::new("invalid", "A step can't be empty."));
    }
    let (step, topic_id) = {
        let state = app.state::<AppState>();
        let conn = lock_db(&state);
        db::update_step_content(&conn, &step_id, &content, &now_iso())?;
        db::delete_ledger_for_source(&conn, &step_id)?;
        let step = db::get_step(&conn, &step_id)?;
        let topic_id = step.topic_id.clone();
        (step, topic_id)
    };
    emit_ledger_state(&app, &topic_id);
    spawn_ledger_extraction(app, topic_id, "spine".to_string(), step_id, content);
    Ok(step)
}

// ----------------------------------------------------------- side notes --

#[tauri::command]
pub fn create_side_note(
    app: AppHandle,
    topic_id: String,
    step_id: String,
    start_offset: i64,
    end_offset: i64,
    quoted_text: String,
    question: String,
) -> Result<NoteGenStarted, CmdError> {
    auth::require_key()?;
    let question = question.trim().to_string();
    if question.is_empty() {
        return Err(CmdError::new("invalid", "Ask a question about the highlight."));
    }
    let quoted = quoted_text.trim().to_string();
    if quoted.is_empty() {
        return Err(CmdError::new("invalid", "Nothing is highlighted."));
    }

    let note_id = new_id();
    let (note, request) = {
        let state = app.state::<AppState>();
        let conn = lock_db(&state);
        let step = db::get_step(&conn, &step_id)?;
        if step.topic_id != topic_id {
            return Err(CmdError::new("invalid", "That step belongs to another topic."));
        }
        let topic = db::get_topic_row(&conn, &topic_id)?;
        let settings = db::get_settings(&conn)?;
        let now = now_iso();
        db::insert_side_note(
            &conn,
            &note_id,
            &topic_id,
            &step_id,
            start_offset,
            end_offset,
            &quoted,
            &now,
        )?;
        db::insert_note_message(&conn, &new_id(), &note_id, "user", &question, &now)?;
        let note = db::get_note(&conn, &note_id)?;
        let request = prompts::build_side_note_request(
            &settings,
            &topic.title,
            &step.content,
            &quoted,
            &note.messages,
        );
        (note, request)
    };

    let gen_id = new_id();
    spawn_note_generation(app, gen_id.clone(), topic_id, note_id, request);
    Ok(NoteGenStarted { gen_id, note })
}

#[tauri::command]
pub fn reply_side_note(
    app: AppHandle,
    note_id: String,
    content: String,
) -> Result<NoteGenStarted, CmdError> {
    auth::require_key()?;
    let content = content.trim().to_string();
    if content.is_empty() {
        return Err(CmdError::new("invalid", "The reply is empty."));
    }

    let (note, request, topic_id) = {
        let state = app.state::<AppState>();
        let conn = lock_db(&state);
        let existing = db::get_note(&conn, &note_id)?;
        let step = db::get_step(&conn, &existing.anchor_step_id)?;
        let topic = db::get_topic_row(&conn, &existing.topic_id)?;
        let settings = db::get_settings(&conn)?;
        db::insert_note_message(&conn, &new_id(), &note_id, "user", &content, &now_iso())?;
        let note = db::get_note(&conn, &note_id)?;
        let request = prompts::build_side_note_request(
            &settings,
            &topic.title,
            &step.content,
            &note.quoted_text,
            &note.messages,
        );
        let topic_id = note.topic_id.clone();
        (note, request, topic_id)
    };

    let gen_id = new_id();
    spawn_note_generation(app, gen_id.clone(), topic_id, note_id, request);
    Ok(NoteGenStarted { gen_id, note })
}

/// Re-runs generation for a note whose last message is an unanswered user
/// question (used by the retry path after a failed generation).
#[tauri::command]
pub fn retry_side_note(app: AppHandle, note_id: String) -> Result<NoteGenStarted, CmdError> {
    auth::require_key()?;
    let (note, request, topic_id) = {
        let state = app.state::<AppState>();
        let conn = lock_db(&state);
        let note = db::get_note(&conn, &note_id)?;
        if note.messages.last().map(|m| m.role.as_str()) != Some("user") {
            return Err(CmdError::new("invalid", "Nothing to retry."));
        }
        let step = db::get_step(&conn, &note.anchor_step_id)?;
        let topic = db::get_topic_row(&conn, &note.topic_id)?;
        let settings = db::get_settings(&conn)?;
        let request = prompts::build_side_note_request(
            &settings,
            &topic.title,
            &step.content,
            &note.quoted_text,
            &note.messages,
        );
        let topic_id = note.topic_id.clone();
        (note, request, topic_id)
    };
    let gen_id = new_id();
    spawn_note_generation(app, gen_id.clone(), topic_id, note_id, request);
    Ok(NoteGenStarted { gen_id, note })
}

#[tauri::command]
pub fn set_side_note_resolved(
    app: AppHandle,
    note_id: String,
    resolved: bool,
) -> Result<SideNote, CmdError> {
    let (note, was_unresolved) = {
        let state = app.state::<AppState>();
        let conn = lock_db(&state);
        let before = db::get_note(&conn, &note_id)?;
        db::set_note_resolved(&conn, &note_id, resolved)?;
        let note = db::get_note(&conn, &note_id)?;
        (note, !before.resolved)
    };
    // Resolving a note feeds its clarification into the concept ledger
    // (§6.3) so spine generation won't re-teach it.
    if resolved && was_unresolved && !note.messages.is_empty() {
        let thread = note
            .messages
            .iter()
            .map(|m| {
                let who = if m.role == "user" { "Q" } else { "A" };
                format!("{who}: {}", m.content)
            })
            .collect::<Vec<_>>()
            .join("\n");
        let excerpt = format!(
            "Clarification about \"{}\":\n{}",
            note.quoted_text, thread
        );
        spawn_ledger_extraction(
            app,
            note.topic_id.clone(),
            "side_note".to_string(),
            note_id,
            excerpt,
        );
    }
    Ok(note)
}

#[tauri::command]
pub fn delete_side_note(state: State<'_, AppState>, note_id: String) -> Result<(), CmdError> {
    let conn = lock_db(&state);
    // concept_ledger.source_id is polymorphic (step or note), so it can't
    // carry a foreign key — the note's ledger rows must be cleared by hand,
    // or they keep telling spine generation that concept is already covered.
    db::delete_ledger_for_source(&conn, &note_id)?;
    db::delete_note(&conn, &note_id)?;
    Ok(())
}

#[tauri::command]
pub fn update_side_note_anchor(
    state: State<'_, AppState>,
    note_id: String,
    start_offset: i64,
    end_offset: i64,
) -> Result<(), CmdError> {
    let conn = lock_db(&state);
    db::update_note_anchor(&conn, &note_id, start_offset, end_offset)?;
    Ok(())
}

// -------------------------------------------------------- import/export --

fn sanitize_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '-',
            c if (c as u32) < 0x20 => '-',
            c => c,
        })
        .collect();
    let trimmed = cleaned.trim().trim_matches('.').to_string();
    if trimmed.is_empty() {
        "topic".to_string()
    } else {
        trimmed
    }
}

/// Diagrams pulled out of an export, written as files beside the Markdown.
struct ExportDiagrams {
    /// Folder name, relative to the Markdown file.
    dir: String,
    /// (file name, standalone SVG)
    files: Vec<(String, String)>,
}

impl ExportDiagrams {
    /// `content` with each drawable svg block replaced by an image link to
    /// its own file. Blocks that won't sanitize stay as code.
    fn link(&mut self, content: &str) -> String {
        markdown::replace_svg_blocks(content, |block| {
            let svg = svg::standalone(&block.source)?;
            let name = format!("diagram-{}.svg", self.files.len() + 1);
            let link = format!("![Diagram](<{}/{name}>)\n", self.dir);
            self.files.push((name, svg));
            Some(link)
        })
    }
}

fn build_export_markdown(t: &TopicDetail, diagrams: &mut ExportDiagrams) -> String {
    let mut out = String::new();
    out.push_str(&format!("# {}\n\n", t.title));
    out.push_str(&format!(
        "*A Waypoint learning path, exported {}.*\n\n",
        Utc::now().format("%Y-%m-%d")
    ));

    let mut footnotes: Vec<String> = Vec::new();
    let mut n = 0usize;

    for step in &t.steps {
        out.push_str("---\n\n");
        if let Some(p) = &step.prompt {
            out.push_str(&format!("> *Steered: {}*\n\n", p.replace('\n', " ")));
        }
        let mut content = diagrams.link(&step.content);
        let notes: Vec<&SideNote> = t
            .side_notes
            .iter()
            .filter(|note| note.anchor_step_id == step.id && note.resolved)
            .collect();
        let mut trailing: Vec<usize> = Vec::new();
        for note in notes {
            n += 1;
            let marker = format!("[^{n}]");
            match content.find(note.quoted_text.as_str()) {
                Some(idx) => content.insert_str(idx + note.quoted_text.len(), &marker),
                None => trailing.push(n),
            }
            let thread = note
                .messages
                .iter()
                .map(|m| {
                    let who = if m.role == "user" { "Q" } else { "A" };
                    format!("**{who}:** {}", diagrams.link(&m.content).replace('\n', " "))
                })
                .collect::<Vec<_>>()
                .join(" \u{2022} ");
            footnotes.push(format!(
                "[^{n}]: **\u{201c}{}\u{201d}**: {thread}",
                note.quoted_text.replace('\n', " ")
            ));
        }
        out.push_str(&content);
        out.push('\n');
        if !trailing.is_empty() {
            let refs = trailing
                .iter()
                .map(|i| format!("[^{i}]"))
                .collect::<Vec<_>>()
                .join(" ");
            out.push_str(&format!("\n*Side notes: {refs}*\n"));
        }
        out.push('\n');
    }

    if !footnotes.is_empty() {
        out.push_str("---\n\n## Side notes\n\n");
        for f in &footnotes {
            out.push_str(f);
            out.push_str("\n\n");
        }
    }

    if !t.ledger.is_empty() {
        out.push_str("---\n\n## Concepts covered\n\n");
        for c in &t.ledger {
            out.push_str(&format!("- {}\n", c.label));
        }
    }
    out
}

#[tauri::command]
pub async fn export_topic_markdown(
    app: AppHandle,
    topic_id: String,
) -> Result<Option<String>, CmdError> {
    let (detail, filename) = {
        let state = app.state::<AppState>();
        let conn = lock_db(&state);
        let detail = db::get_topic_detail(&conn, &topic_id)?;
        let filename = format!("{}.md", sanitize_filename(&detail.title));
        (detail, filename)
    };
    let path = tauri::async_runtime::spawn_blocking(move || {
        rfd::FileDialog::new()
            .set_title("Export topic as Markdown")
            .set_file_name(filename)
            .add_filter("Markdown", &["md"])
            .save_file()
    })
    .await
    .map_err(|e| CmdError::new("io", e.to_string()))?;
    let Some(p) = path else { return Ok(None) };

    // Diagrams go in a folder named after the file the user actually chose.
    let stem = p
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "topic".to_string());
    let mut diagrams = ExportDiagrams {
        dir: format!("{stem}-diagrams"),
        files: Vec::new(),
    };
    let md = build_export_markdown(&detail, &mut diagrams);
    std::fs::write(&p, md)?;
    if !diagrams.files.is_empty() {
        let dir = p.with_file_name(&diagrams.dir);
        std::fs::create_dir_all(&dir)?;
        for (name, svg) in &diagrams.files {
            std::fs::write(dir.join(name), svg)?;
        }
    }
    Ok(Some(p.display().to_string()))
}

#[tauri::command]
pub async fn import_documents() -> Result<Vec<ImportedFile>, CmdError> {
    let all: Vec<&str> = documents::TEXT_EXTS
        .iter()
        .chain(documents::DOC_EXTS.iter())
        .copied()
        .collect();

    let picked = tauri::async_runtime::spawn_blocking(move || {
        rfd::FileDialog::new()
            .set_title("Attach documents as topic context")
            .add_filter("Documents", &all)
            .add_filter("PDF", documents::DOC_EXTS)
            .add_filter("All files", &["*"])
            .pick_files()
    })
    .await
    .map_err(|e| CmdError::new("io", e.to_string()))?;

    read_paths(picked.unwrap_or_default()).await
}

/// Reads files dropped onto the window. Unreadable ones are reported rather
/// than failing the whole drop, so one bad file in a batch isn't fatal.
#[tauri::command]
pub async fn read_dropped_files(paths: Vec<String>) -> Result<Vec<ImportedFile>, CmdError> {
    read_paths(paths.into_iter().map(PathBuf::from).collect()).await
}

async fn read_paths(paths: Vec<PathBuf>) -> Result<Vec<ImportedFile>, CmdError> {
    if paths.is_empty() {
        return Ok(Vec::new());
    }
    tauri::async_runtime::spawn_blocking(move || {
        paths
            .into_iter()
            .filter(|p| p.is_file())
            .map(|p| {
                let name = documents::display_name(&p);
                match documents::extract(&p) {
                    Ok(content) => ImportedFile {
                        name,
                        content,
                        error: None,
                    },
                    Err(e) => ImportedFile {
                        name,
                        content: String::new(),
                        error: Some(e.message),
                    },
                }
            })
            .collect()
    })
    .await
    .map_err(|e| CmdError::new("io", e.to_string()))
}

// ------------------------------------------------------- backup/restore --

#[tauri::command]
pub async fn backup_database(app: AppHandle) -> Result<Option<String>, CmdError> {
    let default_name = format!(
        "waypoint-backup-{}.db",
        Utc::now().format("%Y%m%d-%H%M%S")
    );
    let path = tauri::async_runtime::spawn_blocking(move || {
        rfd::FileDialog::new()
            .set_title("Back up Waypoint data")
            .set_file_name(default_name)
            .add_filter("SQLite database", &["db"])
            .save_file()
    })
    .await
    .map_err(|e| CmdError::new("io", e.to_string()))?;

    let Some(dest) = path else { return Ok(None) };
    // Overwriting a stale backup file could leave mixed pages; start clean.
    let _ = std::fs::remove_file(&dest);

    let state = app.state::<AppState>();
    let conn = lock_db(&state);
    let mut dst = rusqlite::Connection::open(&dest)?;
    let backup = rusqlite::backup::Backup::new(&conn, &mut dst)?;
    backup.run_to_completion(64, Duration::from_millis(20), None)?;
    drop(backup);
    Ok(Some(dest.display().to_string()))
}

#[tauri::command]
pub async fn restore_database(app: AppHandle) -> Result<Option<bool>, CmdError> {
    let picked = tauri::async_runtime::spawn_blocking(|| {
        rfd::FileDialog::new()
            .set_title("Restore Waypoint data from a backup")
            .add_filter("SQLite database", &["db"])
            .pick_file()
    })
    .await
    .map_err(|e| CmdError::new("io", e.to_string()))?;

    let Some(src) = picked else { return Ok(None) };

    // Validate that the picked file is actually a Waypoint database.
    {
        let check = rusqlite::Connection::open_with_flags(
            &src,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .map_err(|_| CmdError::new("invalid", "That file isn't a database."))?;
        let count: i64 = check
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' \
                 AND name IN ('topics', 'spine_steps', 'side_notes')",
                [],
                |r| r.get(0),
            )
            .map_err(|_| CmdError::new("invalid", "That file isn't a Waypoint backup."))?;
        if count < 3 {
            return Err(CmdError::new(
                "invalid",
                "That file doesn't look like a Waypoint backup.",
            ));
        }
    }

    let state = app.state::<AppState>();
    let db_path = state.db_path.clone();
    let safety = db_path.with_extension("pre-restore.bak");
    {
        let mut guard = lock_db(&state);

        // Keep a safety copy of the current data before overwriting anything.
        let _ = std::fs::remove_file(&safety);
        {
            let mut dst = rusqlite::Connection::open(&safety)?;
            let b = rusqlite::backup::Backup::new(&guard, &mut dst)?;
            b.run_to_completion(64, Duration::from_millis(20), None)?;
        }

        // Release the live database file, swap in the backup, reopen.
        *guard = rusqlite::Connection::open_in_memory()?;
        let _ = std::fs::remove_file(db_path.with_extension("db-wal"));
        let _ = std::fs::remove_file(db_path.with_extension("db-shm"));
        std::fs::copy(&src, &db_path)?;
        match db::open(&db_path) {
            Ok(c) => *guard = c,
            Err(e) => {
                // Roll back to the safety copy so the app keeps working.
                let _ = std::fs::copy(&safety, &db_path);
                *guard = db::open(&db_path)?;
                return Err(CmdError::new(
                    "invalid",
                    format!("Restore failed; your previous data was kept. ({e})"),
                ));
            }
        }
    }
    let _ = app.emit("db:restored", ());
    Ok(Some(true))
}
