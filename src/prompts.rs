//! Context construction for the two generation modes (§6 of the design doc).
//!
//! Spine generation gets: prior spine history (truncated once long) + the
//! concept ledger + optional steering. It never sees side-note transcripts.
//! Side-note generation gets: the anchor step + highlighted text + its own
//! short thread. It never sees the rest of the spine.

use crate::anthropic::{ChatMessage, MessagesRequest, SystemBlock};
use crate::models::{Settings, SideNoteMessage, SpineStep};

/// Full spine steps included verbatim in context; older ones are represented
/// only by the concept ledger.
const MAX_HISTORY_STEPS: usize = 24;
/// The seed is resent on every step, but it sits in the cached prefix, so
/// resends bill at the cache-read rate rather than full price. This is now a
/// context-window guard rather than a cost guard. Kept in sync with
/// SEED_BUDGET_CHARS in ui/js/views/topics.js.
const MAX_SEED_CHARS: usize = 200_000;
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

/// Seed context long enough to be worth caching. Below the model's minimum
/// cacheable prefix the marker is simply ignored, so this only avoids paying
/// the write premium on a prefix that could never be reused.
const MIN_CACHEABLE_SEED_CHARS: usize = 4000;

/// The system prompt in two parts: everything stable for the life of the
/// topic, then the concept ledger, which changes after every step. The split
/// is what makes caching possible — a single block ending in the ledger would
/// differ on every call and never hit the cache.
fn spine_system(
    title: &str,
    seed: Option<&str>,
    size_line: &str,
    ledger: &[String],
) -> Vec<SystemBlock> {
    let mut s = String::new();
    s.push_str("You are Waypoint, a tutor that builds understanding one deliberate step at a time.\n\n");
    s.push_str(&format!("Topic being learned: {title}\n"));

    let mut cacheable = false;
    if let Some(seed) = seed {
        let seed = truncate_chars(seed, MAX_SEED_CHARS);
        cacheable = seed.len() >= MIN_CACHEABLE_SEED_CHARS;
        s.push_str("\nThe learner supplied this source material as starting context. Ground the path in it where relevant:\n<source_material>\n");
        s.push_str(&seed);
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

    let mut volatile = String::new();
    volatile.push_str("Concepts already covered (including ones clarified in side notes):\n");
    if ledger.is_empty() {
        volatile.push_str("(none yet \u{2014} this is the beginning of the path)\n");
    } else {
        for label in ledger {
            volatile.push_str(&format!("- {label}\n"));
        }
    }

    vec![
        if cacheable {
            SystemBlock::stable(s)
        } else {
            SystemBlock::volatile(s)
        },
        SystemBlock::volatile(volatile),
    ]
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
        system: vec![SystemBlock::volatile(system)],
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
        system: vec![SystemBlock::volatile(system)],
        messages: vec![ChatMessage::user(user)],
        effort: None,
    }
}

/// Recent steps quoted for the quiz. Fewer than the spine's history budget —
/// a question should test what was actually taught, not the whole topic.
const QUIZ_STEPS: usize = 8;

pub fn build_quiz_request(
    settings: &Settings,
    title: &str,
    steps: &[SpineStep],
    ledger: &[String],
    already_asked: &[String],
) -> MessagesRequest {
    let mut system = String::new();
    system.push_str("You write a single multiple-choice question that checks whether a learner understood what they were just taught.\n\n");
    system.push_str(&format!("Topic being learned: {title}\n\n"));
    system.push_str("Rules:\n");
    system.push_str("- Test understanding, not recall of wording. A learner who followed the material should be able to reason it out; one who skimmed should not.\n");
    system.push_str("- Exactly four options. Exactly one is correct.\n");
    system.push_str("- Wrong options must be plausible and reflect real misconceptions about this material \u{2014} never filler, jokes, or obviously absurd choices.\n");
    system.push_str("- Only ask about material actually covered below.\n");
    system.push_str("- Keep the question and each option to one sentence. Use $ \u{2026} $ for any mathematics.\n");
    system.push_str("- The explanation says why the right answer is right in one or two sentences, and never mentions option letters or numbers.\n\n");
    system.push_str("Respond with ONLY a JSON object, no prose and no code fences:\n");
    system.push_str(r#"{"question": "...", "options": ["...", "...", "...", "..."], "correct_index": 0, "explanation": "..."}"#);

    let mut user = String::new();
    if !ledger.is_empty() {
        user.push_str("Concepts covered so far:\n");
        for label in ledger {
            user.push_str(&format!("- {label}\n"));
        }
        user.push('\n');
    }
    user.push_str("The most recent lesson steps:\n");
    let start = steps.len().saturating_sub(QUIZ_STEPS);
    for (i, step) in steps[start..].iter().enumerate() {
        user.push_str(&format!(
            "<step n=\"{}\">\n{}\n</step>\n",
            start + i + 1,
            truncate_chars(&step.content, MAX_EXCERPT_CHARS)
        ));
    }
    if !already_asked.is_empty() {
        user.push_str("\nAsk about something different from these, which have already been asked:\n");
        for q in already_asked {
            user.push_str(&format!("- {q}\n"));
        }
    }

    MessagesRequest {
        model: settings.model.clone(),
        max_tokens: 2000,
        system: vec![SystemBlock::volatile(system)],
        messages: vec![ChatMessage::user(user)],
        effort: effort_for(&settings.model, "low"),
    }
}

/// Extracts the JSON object from the model's reply. Returns None when the
/// shape isn't a usable four-option question.
pub fn parse_quiz(text: &str) -> Option<(String, Vec<String>, usize, String)> {
    let t = text.trim();
    let (start, end) = (t.find('{')?, t.rfind('}')?);
    if end <= start {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(&t[start..=end]).ok()?;

    let question = v["question"].as_str()?.trim().to_string();
    let options: Vec<String> = v["options"]
        .as_array()?
        .iter()
        .filter_map(|o| o.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let correct = v["correct_index"].as_u64()? as usize;
    let explanation = v["explanation"].as_str().unwrap_or("").trim().to_string();

    if question.is_empty() || options.len() != 4 || correct >= options.len() {
        return None;
    }
    Some((question, options, correct, explanation))
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
        system: Vec::new(),
        messages: vec![ChatMessage::user("Hi")],
        effort: None,
    }
}
