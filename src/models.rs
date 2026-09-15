use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
pub struct TopicSummary {
    pub id: String,
    pub title: String,
    pub status: String,
    pub position: i64,
    pub created_at: String,
    pub updated_at: String,
    pub step_count: i64,
    pub open_note_count: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_usd: f64,
}

/// Bare topic row, used internally when building prompts.
#[derive(Debug, Clone)]
pub struct TopicRow {
    pub id: String,
    pub title: String,
    pub status: String,
    pub seed_context: Option<String>,
    /// The learner's own description of what they already understand.
    pub prior_knowledge: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SpineStep {
    pub id: String,
    pub topic_id: String,
    pub position: i64,
    /// The user's steering text, if any; None = "AI decided".
    pub prompt: Option<String>,
    /// Markdown source of the AI's response.
    pub content: String,
    /// Rendered (sanitized) HTML of `content`.
    pub html: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SideNoteMessage {
    pub id: String,
    pub position: i64,
    pub role: String,
    pub content: String,
    pub html: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SideNote {
    pub id: String,
    pub topic_id: String,
    pub anchor_step_id: String,
    /// Char offsets into the *plain text* rendering of the anchor step's
    /// content. The quoted_text snapshot is the source of truth for
    /// re-anchoring if the content is edited or regenerated.
    pub start_offset: i64,
    pub end_offset: i64,
    pub quoted_text: String,
    pub resolved: bool,
    pub created_at: String,
    pub messages: Vec<SideNoteMessage>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ConceptEntry {
    pub id: String,
    pub topic_id: String,
    pub label: String,
    /// "spine" | "side_note"
    pub source_kind: String,
    pub source_id: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct UsageTotals {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct TopicDetail {
    pub id: String,
    pub title: String,
    pub status: String,
    pub seed_context: Option<String>,
    pub prior_knowledge: Option<String>,
    pub created_at: String,
    pub steps: Vec<SpineStep>,
    pub side_notes: Vec<SideNote>,
    pub ledger: Vec<ConceptEntry>,
    pub usage: UsageTotals,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default = "default_step_size")]
    pub step_size: String,
    #[serde(default = "default_theme")]
    pub theme: String,
}

fn default_model() -> String {
    "claude-opus-5".to_string()
}
fn default_step_size() -> String {
    "standard".to_string()
}
fn default_theme() -> String {
    "system".to_string()
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            model: default_model(),
            step_size: default_step_size(),
            theme: default_theme(),
        }
    }
}
