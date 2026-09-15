//! Context construction for the two generation modes (§6 of the design doc).
//!
//! Spine generation gets: prior spine history (truncated once long) + the
//! concept ledger + optional steering. It never sees side-note transcripts.
//! Side-note generation gets: the anchor step + highlighted text + its own
//! short thread. It never sees the rest of the spine.

use base64::Engine;
use serde_json::json;

use crate::anthropic::{ChatMessage, MessageContent, MessagesRequest, SystemBlock};
use crate::markdown;
use crate::models::{Settings, SideNoteMessage, SpineStep};
use crate::svg;

/// Full spine steps included verbatim in context; older ones are represented
/// only by the concept ledger.
const MAX_HISTORY_STEPS: usize = 24;
/// The seed is resent on every step, but it sits in the cached prefix, so
/// resends bill at the cache-read rate rather than full price. This is now a
/// context-window guard rather than a cost guard. Kept in sync with
/// SEED_BUDGET_CHARS in ui/js/views/topics.js.
const MAX_SEED_CHARS: usize = 200_000;
const MAX_EXCERPT_CHARS: usize = 6000;
/// Kept in sync with PRIOR_KNOWLEDGE_MAX_CHARS in ui/js/views/topics.js.
const MAX_PRIOR_KNOWLEDGE_CHARS: usize = 4000;

/// Used in place of DIAGRAM_RULES while the experimental setting is off.
const DIAGRAMS_OFF: &str = "Diagrams are turned off. Never write SVG, a ```svg block, or any other drawing code, even if the learner asks. If the learner asks for a diagram, picture or visual, say in one sentence that diagrams are an experimental feature they can turn on in Settings under Experimental, then explain in words as clearly as you can.";

/// How to write diagrams that survive the sanitizer in svg.rs.
const DIAGRAM_RULES: &str = r##"- When a picture would genuinely help (geometry, light paths, vectors, plots, structures, processes), you may include one diagram as a ```svg code block containing a single <svg> element. Most steps need no diagram.
  Give it a viewBox about 600 units wide and no width or height. Use shapes, paths, text, markers, gradients and clip paths only: scripts, images, foreignObject, links and <style> elements are stripped.
  Colour with currentColor, which defaults to the lesson's text colour. Put class="blue" (or red, orange, yellow, green, purple, accent, muted) on an element or group to change currentColor, and use fill-opacity for soft fills. class="paper" is the page background, for backdrops behind labels. Never hardcode black, white or a background rectangle, so the diagram works in light and dark themes.
  Labels can use $...$ LaTeX: put the whole label in one <text> element with no <tspan> children, positioned with x, y, text-anchor and dominant-baseline as usual. Plain labels without math are fine as ordinary text.
  SVG's y axis points down. A direction that points up on screen has negative dy, a rotate() with a positive angle turns clockwise on screen, and in an arc command (A) sweep-flag 1 means clockwise on screen. If you reason in math coordinates with y up, flip the sign of every y offset and every angle before writing the SVG.
  Compute coordinates numerically rather than estimating them. Derive every point from the points it depends on: an arc or tick that marks something about two segments should start and end at points computed along those segments, and a label placed beside a shape should be offset from that shape's computed position. Before finishing, check that each drawn element actually lines up with what it refers to.
  Place every label in clear space: at least half a font size away from any line, arrowhead or other label. Put a line's label beside its midpoint or just past its end, offset perpendicular to the line and away from other elements, and use text-anchor so the text grows away from what it labels. Estimate each label's width (about 0.55 font-size per character, typeset math a little wider) and check that box against nearby lines before settling on a position. Keep the picture uncluttered.
"##;

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
            "Keep the step to 1\u{2013}2 short paragraphs, at most about 120 words.",
            3000,
        ),
        "deep" => (
            "The step may run 3\u{2013}5 short paragraphs (about 400 words). Include a short worked example when it genuinely clarifies the concept.",
            5000,
        ),
        _ => (
            "Keep the step to 2\u{2013}3 short paragraphs, at most about 220 words.",
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
    prior_knowledge: Option<&str>,
    size_line: &str,
    diagrams: bool,
    ledger: &[String],
) -> Vec<SystemBlock> {
    let mut s = String::new();
    s.push_str("You are Waypoint, a tutor that builds understanding one deliberate step at a time.\n\n");
    s.push_str(&format!("Topic being learned: {title}\n"));

    if let Some(prior) = prior_knowledge {
        s.push_str("\nThe learner described what they already understand. Start the path from there: do not teach these foundations from scratch, pitch the first steps at this level, and build on what they know by name. If the description is vague, assume only what it clearly states.\n<learner_background>\n");
        s.push_str(&truncate_chars(prior, MAX_PRIOR_KNOWLEDGE_CHARS));
        s.push_str("\n</learner_background>\n");
    }

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
    if diagrams {
        s.push_str(DIAGRAM_RULES);
    } else {
        s.push_str("- ");
        s.push_str(DIAGRAMS_OFF);
        s.push('\n');
    }
    s.push_str("- Never use em dashes (\u{2014}). Use commas, colons, parentheses, or separate sentences instead.\n");
    s.push_str("- Do not preview or promise future steps; never end with \"next we will\u{2026}\".\n");
    s.push_str("- Do not re-explain concepts already covered (listed below). Build on them by name instead.\n");
    s.push_str("- If the learner steers the step with an instruction, follow it while keeping the response one focused step.\n");

    let mut volatile = String::new();
    volatile.push_str("Concepts already covered (including ones clarified in side notes):\n");
    if ledger.is_empty() {
        volatile.push_str("(none yet; this is the beginning of the path)\n");
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

/// Content as the model should see it: with diagrams off, earlier diagrams
/// are removed so it doesn't keep imitating them.
fn visible_content(content: &str, diagrams: bool) -> String {
    if diagrams {
        content.to_string()
    } else {
        markdown::strip_svg_blocks(content)
    }
}

fn history_messages(steps: &[SpineStep], diagrams: bool) -> Vec<ChatMessage> {
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
        msgs.push(ChatMessage::assistant(visible_content(&step.content, diagrams)));
    }
    msgs
}

pub fn build_spine_request(
    settings: &Settings,
    title: &str,
    seed: Option<&str>,
    prior_knowledge: Option<&str>,
    steps: &[SpineStep],
    ledger: &[String],
    steering: Option<&str>,
) -> MessagesRequest {
    let (size_line, max_tokens) = step_size(&settings.step_size);
    let mut messages = history_messages(steps, settings.diagrams);
    messages.push(ChatMessage::user(
        steering.map(str::to_string).unwrap_or_else(|| "Continue.".to_string()),
    ));
    MessagesRequest {
        model: settings.model.clone(),
        max_tokens,
        system: spine_system(title, seed, prior_knowledge, size_line, settings.diagrams, ledger),
        messages,
        effort: effort_for(&settings.model, "medium"),
    }
}

pub fn build_regen_request(
    settings: &Settings,
    title: &str,
    seed: Option<&str>,
    prior_knowledge: Option<&str>,
    prior_steps: &[SpineStep],
    ledger: &[String],
    steering: Option<&str>,
    previous_content: &str,
) -> MessagesRequest {
    let (size_line, max_tokens) = step_size(&settings.step_size);
    let mut messages = history_messages(prior_steps, settings.diagrams);
    let mut ask = String::from("Regenerate this step. The previous version missed the mark.");
    if let Some(s) = steering {
        ask.push_str(&format!(" Instruction: {s}"));
    }
    ask.push_str("\n\nThe previous version is below. Take a meaningfully different or improved angle rather than repeating it.\n<previous_version>\n");
    ask.push_str(&visible_content(previous_content, settings.diagrams));
    ask.push_str("\n</previous_version>");
    messages.push(ChatMessage::user(ask));
    MessagesRequest {
        model: settings.model.clone(),
        max_tokens,
        system: spine_system(title, seed, prior_knowledge, size_line, settings.diagrams, ledger),
        messages,
        effort: effort_for(&settings.model, "medium"),
    }
}

pub fn build_side_note_request(
    settings: &Settings,
    topic_title: &str,
    step_content: &str,
    quoted_text: &str,
    thread: &[SideNoteMessage],
) -> MessagesRequest {
    let mut system = String::new();
    system.push_str("You are Waypoint's side-note assistant. The learner is reading a lesson step and highlighted a specific phrase to ask about it.\n\n");
    system.push_str("Answer only the learner's question about the highlighted text: conversational and concise, one short paragraph unless they explicitly ask for more. Stay scoped to the clarification: do not continue the lesson, introduce the next concept, or restate the whole step.\n\n");
    system.push_str("Never use em dashes (\u{2014}). Use commas, colons, parentheses, or separate sentences instead.\n\n");
    system.push_str("Write mathematics in LaTeX: $ \u{2026} $ for inline math, $$ \u{2026} $$ on its own lines for a displayed equation.\n\n");
    if settings.diagrams {
        system.push_str("Diagrams in side notes should be rare and small: draw one only when the question is about a shape, a geometric arrangement or a relationship that words handle badly. Drawing rules:\n");
        system.push_str(DIAGRAM_RULES);
        system.push('\n');
    } else {
        system.push_str(DIAGRAMS_OFF);
        system.push_str("\n\n");
    }
    system.push_str(&format!("Topic being learned: {topic_title}\n\n"));
    system.push_str("The lesson step the learner is reading:\n<step>\n");
    system.push_str(&truncate_chars(
        &visible_content(step_content, settings.diagrams),
        MAX_EXCERPT_CHARS,
    ));
    system.push_str("\n</step>\n\n");
    system.push_str(&format!("Highlighted text: \"{quoted_text}\"\n"));

    let messages = thread
        .iter()
        .map(|m| ChatMessage {
            role: m.role.clone(),
            content: MessageContent::Text(visible_content(&m.content, settings.diagrams)),
        })
        .collect();

    MessagesRequest {
        model: settings.model.clone(),
        // Room for a diagram's SVG on top of the answer and its thinking.
        max_tokens: if settings.diagrams { 8000 } else { 2500 },
        system: vec![SystemBlock::volatile(system)],
        messages,
        effort: effort_for(&settings.model, "low"),
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

/// Text sent alongside a diagram under review. Generous: it includes the
/// diagram's own source.
const MAX_REVIEW_TEXT_CHARS: usize = 30_000;

pub fn build_diagram_review_request(
    model: &str,
    surrounding_text: &str,
    svg_source: &str,
    png: &[u8],
) -> MessagesRequest {
    let mut system = String::new();
    system.push_str("You review a diagram drawn in a lesson, either in a lesson step or in an answer to a learner's side question. You receive a rendering of the diagram, the text the diagram appears in, and the diagram's SVG source.\n\n");
    system.push_str("Compare the rendering with the source and the text, and look for real defects: elements that should meet, touch or line up but don't; arcs, angle marks, braces or ticks that don't span what they refer to; arrows pointing the wrong way; labels overlapping lines, other labels, or sitting by the wrong element; parts cut off by the viewBox; anything that contradicts the text.\n\n");
    system.push_str("The rendering is a preview in the light theme's colours on white, with $...$ labels shown as a plain-text stand-in about the size of the typeset math. Never treat that typesetting as a defect. Text in the lesson sits on a small page-coloured halo, as in the preview, but a label that covers a line, arrowhead or another label is still a defect: move it into clear space.\n\n");
    system.push_str("If the diagram has no real defects, reply with exactly OK. Otherwise reply with only the corrected diagram as a single ```svg code block: fix the defects and keep everything that was already right. The corrected diagram must follow the same drawing rules:\n");
    system.push_str(DIAGRAM_RULES);

    let text = format!(
        "<text>\n{}\n</text>\n\n<svg_source>\n{svg_source}\n</svg_source>",
        truncate_chars(surrounding_text, MAX_REVIEW_TEXT_CHARS)
    );
    let blocks = vec![
        json!({
            "type": "image",
            "source": {
                "type": "base64",
                "media_type": "image/png",
                "data": base64::engine::general_purpose::STANDARD.encode(png),
            },
        }),
        json!({ "type": "text", "text": text }),
    ];

    MessagesRequest {
        model: model.to_string(),
        max_tokens: 16_000,
        system: vec![SystemBlock::volatile(system)],
        messages: vec![ChatMessage::user_blocks(blocks)],
        effort: effort_for(model, "medium"),
    }
}

/// The corrected SVG from a review reply, or None to keep the original: the
/// reviewer said OK, or its replacement isn't a usable diagram.
pub fn parse_diagram_review(reply: &str, original: &str) -> Option<String> {
    let reply = reply.trim();
    if reply == "OK" {
        return None;
    }
    let fixed = markdown::svg_blocks(reply).into_iter().next()?.source;
    if fixed.trim() == original.trim() || svg::sanitize(&fixed).is_none() {
        return None;
    }
    if original.ends_with('\n') && !fixed.ends_with('\n') {
        return Some(fixed + "\n");
    }
    Some(fixed)
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
