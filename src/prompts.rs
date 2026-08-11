//! Context construction for the two generation modes (§6 of the design doc).
//!
//! Spine generation gets: prior spine history (truncated once long) + the
//! concept ledger + optional steering. It never sees side-note transcripts.
//! Side-note generation gets: the anchor step + highlighted text + its own
//! short thread. It never sees the rest of the spine.

use crate::anthropic::{ChatMessage, MessagesRequest};
use crate::models::{Settings, SideNoteMessage, SpineStep};

/// Full spine steps included verbatim in context; older ones are represented
/// only by the concept ledger.
const MAX_HISTORY_STEPS: usize = 24;
const MAX_SEED_CHARS: usize = 8000;
const MAX_EXCERPT_CHARS: usize = 6000;

/// Cheap model used for concept-ledger extraction and the key-test ping.
pub const LEDGER_MODEL: &str = "claude-haiku-4-5";

/// Haiku 4.5 rejects `output_config.effort`; the current Opus/Sonnet models
/// accept it.
fn effort_for(model: &str, level: &str) -> Option<String> {
    if model.contains("haiku") {
        None
    } else {
        Some(level.to_string())
    }
}

/// (instruction line, max_tokens). max_tokens is generous because on current
/// models it caps adaptive thinking *plus* the visible answer.
fn step_size(step_size: &str) -> (&'static str, u32) {
    match step_size {
        "brief" => (
            "Keep the step to 1\u{2013}2 short paragraphs \u{2014} at most about 120 words.",
            3000,
        ),
        "deep" => (
            "The step may run 3\u{2013}5 short paragraphs (about 400 words). Include a short worked example when it genuinely clarifies the concept.",
            5000,
        ),
        _ => (
            "Keep the step to 2\u{2013}3 short paragraphs \u{2014} at most about 220 words.",
            4000,
        ),
    }
}

fn truncate_chars(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let t: String = s.chars().take(max).collect();
        format!("{t}\n[\u{2026} trimmed for length]")
    }
}

fn spine_system(
    title: &str,
    seed: Option<&str>,
    size_line: &str,
    ledger: &[String],
) -> String {
    let mut s = String::new();
    s.push_str("You are Waypoint, a tutor that builds understanding one deliberate step at a time.\n\n");
    s.push_str(&format!("Topic being learned: {title}\n"));

    if let Some(seed) = seed {
        s.push_str("\nThe learner supplied this source material as starting context. Ground the path in it where relevant:\n<source_material>\n");
        s.push_str(&truncate_chars(seed, MAX_SEED_CHARS));
        s.push_str("\n</source_material>\n");
    }

    s.push_str("\nRules for every step:\n");
    s.push_str("- Each response is exactly one step: it teaches a single new concept that builds toward understanding the topic. Never bundle several concepts.\n");
    s.push_str(&format!("- {size_line}\n"));
    s.push_str("- Plain, precise language. Prefer concrete intuition before formalism.\n");
    s.push_str("- Use Markdown sparingly: bold for a newly introduced term, occasional lists or inline `code`. No headings, no horizontal rules, no closing summary.\n");
    s.push_str("- Write mathematics in LaTeX: $ \u{2026} $ for inline math, $$ \u{2026} $$ on its own lines for a displayed equation. Use it whenever a formula is clearer than prose, and define each symbol you introduce.\n");
    s.push_str("- Do not preview or promise future steps; never end with \"next we will\u{2026}\".\n");
    s.push_str("- Do not re-explain concepts already covered (listed below). Build on them by name instead.\n");
    s.push_str("- If the learner steers the step with an instruction, follow it while keeping the response one focused step.\n");

    s.push_str("\nConcepts already covered (including ones clarified in side notes):\n");
    if ledger.is_empty() {
        s.push_str("(none yet \u{2014} this is the beginning of the path)\n");
    } else {
        for label in ledger {
            s.push_str(&format!("- {label}\n"));
        }
    }
    s
}

fn history_messages(steps: &[SpineStep]) -> Vec<ChatMessage> {
    let mut msgs = Vec::new();
    let start = steps.len().saturating_sub(MAX_HISTORY_STEPS);
    if start > 0 {
        msgs.push(ChatMessage::user(
            "(Earlier steps are omitted here; the concept list in your instructions covers what they taught.)",
        ));
    }
    for step in &steps[start..] {
        msgs.push(ChatMessage::user(
            step.prompt.clone().unwrap_or_else(|| "Continue.".to_string()),
        ));
        msgs.push(ChatMessage::assistant(step.content.clone()));
    }
    msgs
}

pub fn build_spine_request(
    settings: &Settings,
    title: &str,
    seed: Option<&str>,
    steps: &[SpineStep],
    ledger: &[String],
    steering: Option<&str>,
) -> MessagesRequest {
    let (size_line, max_tokens) = step_size(&settings.step_size);
    let mut messages = history_messages(steps);
    messages.push(ChatMessage::user(
        steering.map(str::to_string).unwrap_or_else(|| "Continue.".to_string()),
    ));
    MessagesRequest {
        model: settings.model.clone(),
        max_tokens,
        system: spine_system(title, seed, size_line, ledger),
        messages,
        effort: effort_for(&settings.model, "medium"),
    }
}

pub fn build_regen_request(
    settings: &Settings,
    title: &str,
    seed: Option<&str>,
    prior_steps: &[SpineStep],
    ledger: &[String],
    steering: Option<&str>,
    previous_content: &str,
) -> MessagesRequest {
    let (size_line, max_tokens) = step_size(&settings.step_size);
    let mut messages = history_messages(prior_steps);
    let mut ask = String::from("Regenerate this step \u{2014} the previous version missed the mark.");
    if let Some(s) = steering {
        ask.push_str(&format!(" Instruction: {s}"));
    }
    ask.push_str("\n\nThe previous version is below. Take a meaningfully different or improved angle rather than repeating it.\n<previous_version>\n");
    ask.push_str(previous_content);
    ask.push_str("\n</previous_version>");
    messages.push(ChatMessage::user(ask));
    MessagesRequest {
        model: settings.model.clone(),
        max_tokens,
        system: spine_system(title, seed, size_line, ledger),
        messages,
        effort: effort_for(&settings.model, "medium"),
    }
}

pub fn build_side_note_request(
    model: &str,
    topic_title: &str,
    step_content: &str,
    quoted_text: &str,
    thread: &[SideNoteMessage],
) -> MessagesRequest {
    let mut system = String::new();
    system.push_str("You are Waypoint's side-note assistant. The learner is reading a lesson step and highlighted a specific phrase to ask about it.\n\n");
    system.push_str("Answer only the learner's question about the highlighted text: conversational and concise, one short paragraph unless they explicitly ask for more. Stay scoped to the clarification \u{2014} do not continue the lesson, introduce the next concept, or restate the whole step.\n\n");
    system.push_str("Write mathematics in LaTeX: $ \u{2026} $ for inline math, $$ \u{2026} $$ on its own lines for a displayed equation.\n\n");
    system.push_str(&format!("Topic being learned: {topic_title}\n\n"));
    system.push_str("The lesson step the learner is reading:\n<step>\n");
    system.push_str(&truncate_chars(step_content, MAX_EXCERPT_CHARS));
    system.push_str("\n</step>\n\n");
    system.push_str(&format!("Highlighted text: \"{quoted_text}\"\n"));

    let messages = thread
        .iter()
        .map(|m| ChatMessage {
            role: m.role.clone(),
            content: m.content.clone(),
        })
        .collect();

    MessagesRequest {
        model: model.to_string(),
        max_tokens: 2500,
        system,
        messages,
        effort: effort_for(model, "low"),
    }
}

pub fn build_ledger_request(excerpt: &str, existing: &[String]) -> MessagesRequest {
    let system = "You maintain the concept ledger for a learning app. Given a lesson excerpt and the concepts already in the ledger, extract only the genuinely new concepts the excerpt introduces.\n\nRespond with ONLY a JSON array of 0\u{2013}3 short concept labels (2\u{2013}4 words each, lowercase unless a proper noun). No prose, no code fences. Respond with [] if the excerpt introduces nothing new.".to_string();
    let existing_txt = if existing.is_empty() {
        "(empty)".to_string()
    } else {
        existing
            .iter()
            .map(|l| format!("- {l}"))
            .collect::<Vec<_>>()
            .join("\n")
    };
    let user = format!(
        "Concepts already in the ledger:\n{existing_txt}\n\nNew lesson excerpt:\n{}",
        truncate_chars(excerpt, MAX_EXCERPT_CHARS)
    );
    MessagesRequest {
        model: LEDGER_MODEL.to_string(),
        max_tokens: 500,
        system,
        messages: vec![ChatMessage::user(user)],
        effort: None,
    }
}

pub fn parse_ledger_labels(text: &str) -> Vec<String> {
    let t = text.trim();
    let (Some(start), Some(end)) = (t.find('['), t.rfind(']')) else {
        return Vec::new();
    };
    if end <= start {
        return Vec::new();
    }
    serde_json::from_str::<Vec<String>>(&t[start..=end])
        .unwrap_or_default()
        .into_iter()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s.chars().count() <= 60)
        .take(3)
        .collect()
}

/// Cheapest possible request used to validate an API key.
pub fn test_key_request() -> MessagesRequest {
    MessagesRequest {
        model: LEDGER_MODEL.to_string(),
        max_tokens: 1,
        system: String::new(),
        messages: vec![ChatMessage::user("Hi")],
        effort: None,
    }
}
