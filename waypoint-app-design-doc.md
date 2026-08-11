# Waypoint — design doc

*AI-assisted graph-based learning app*

## 1. Summary

Waypoint is a desktop app (Rust) for learning topics through a chat interface structured as a graph rather than a flat transcript. The name reflects the core structure: understanding is built one waypoint at a time along a single deliberate path (the spine), while side notes let you step off briefly to clarify something without losing your place on that path. The user starts a topic (e.g. "ReSTIR"), and the AI delivers understanding in small, deliberate steps — a few short paragraphs at a time — along a single linear **spine**. At any point the user can highlight a confusing phrase and open a lightweight **side note**: a small, scoped clarification thread anchored to that exact text, which does not extend or interrupt the main learning path.

The core design principle: **the spine is the thing being learned; side notes are disposable scaffolding.** They are visually, structurally, and contextually distinct from each other, and the app should never let them blur together.

## 2. Core concepts

### 2.1 Topics
A topic is a self-contained graph (one spine + its side notes). A user may have many topics open in Waypoint's library/sidebar (e.g. "ReSTIR", "Rust async", "General relativity"). Topics do not share state with each other.

### 2.2 Spine
The main path. Strictly linear — a simple ordered sequence of steps, not a tree. Each step is a single AI response of ~2–3 short paragraphs covering exactly one concept, generated either:
- **Unprompted** — user clicks `+` with no text; AI decides the next logical step.
- **Steered** — user clicks `+` and types a short instruction (e.g. "give me an example instead" or "go deeper on visibility reuse").

Design constraint: spine steps must be short and single-concept by construction, not by hope. This is enforced at the prompt level (see §6).

### 2.3 Side notes
A side note is created by highlighting a span of text within any spine step and asking a question about it. It becomes its own small thread (1–3 exchanges typical) anchored to that exact span. Side notes:
- Do not add steps to the spine.
- Are visually subordinate (smaller, muted, collapsible) — margin annotations, not new chat bubbles competing for vertical space.
- Can be marked **resolved**, collapsing to a small marker on the highlighted text.
- Are explicitly *not* forks or branches of the main conversation — they don't imply "an alternate path from here," just "a footnote on this specific thing."

### 2.4 Explicitly out of scope for v1
- Forking/branching the spine into alternate paths (a genuinely different feature from side notes — could be a v2 addition, kept structurally separate from day one so it doesn't get conflated with side notes later).
- Multi-user/collaborative graphs.
- Mobile.

## 3. Data model

```rust
struct Topic {
    id: TopicId,
    title: String,
    spine: Vec<SpineStep>,          // strictly linear, ordered
    side_notes: Vec<SideNote>,      // flat list, each anchored to one spine step
    concept_ledger: Vec<ConceptEntry>, // see §5.3
    created_at: DateTime,
}

struct SpineStep {
    id: NodeId,
    prompt: Option<String>,         // user's steering text, if any; None = "AI decides"
    content: String,                // markdown, the AI's response
    created_at: DateTime,
}

struct SideNote {
    id: NodeId,
    anchor_step_id: NodeId,         // which spine step this is attached to
    anchor: TextAnchor,
    messages: Vec<SideNoteMessage>, // small linear thread, user/AI alternating
    resolved: bool,
}

struct TextAnchor {
    start_offset: usize,            // char offset into anchor_step's content
    end_offset: usize,
    quoted_text: String,            // snapshot of the highlighted text, for
                                     // resilience if content is later edited/re-rendered
}

struct SideNoteMessage {
    role: Role,                     // User | Assistant
    content: String,
    created_at: DateTime,
}

struct ConceptEntry {
    label: String,                  // short concept name, e.g. "temporal reuse"
    source: ConceptSource,          // SpineStep(NodeId) | SideNote(NodeId)
}
```

Key modeling decisions:
- The spine is a `Vec`, not a tree — no branching structure to reason about, no risk of side notes accidentally being modeled as branches.
- `TextAnchor` stores a snapshot of the quoted text, not just offsets, so anchors remain meaningful even if step content is regenerated or edited later.
- `side_notes` is a flat list keyed by `anchor_step_id`, not nested inside `SpineStep` — keeps spine step content small and side notes independently queryable (e.g. "show all unresolved side notes in this topic").

## 4. UI / interaction flow

### 4.1 Main view
Vertical, chat-style, top to bottom — reads like a normal conversation. Each spine step is a full-width bubble.

### 4.2 Advancing the spine
- A `+` control sits below the latest spine step.
- Clicking it opens a small inline input.
- Empty submit → AI picks the next step.
- Text submit → AI uses that as steering instruction for the next step.
- New step animates in below; previous steps stay visible and scrollable above.

### 4.3 Creating a side note
- User selects/highlights a text span inside any spine step (past or current).
- A small popover appears near the selection: "Ask about this."
- Clicking opens a compact panel — suggested placement: slide-in from the right, or an expandable pill in the margin next to the highlighted line.
- The side note's first AI message is auto-seeded with: the highlighted text + a small window of surrounding spine content, so the user doesn't need to re-explain what they're pointing at.
- Side note bubbles are visually distinct from spine bubbles: smaller, muted color, no avatar — should read as a footnote, not a parallel conversation.
- User can continue the side thread for a couple of exchanges, then mark it resolved (or leave it open).
- Resolved side notes collapse to a small marker/dot on the original highlighted text; clicking re-expands them.

### 4.4 Navigation aids (post-MVP, see §7)
- Mini-map: collapsed graph view — spine as a vertical line, side notes as small dots off to the side — for jumping around in long topics.
- Concept index: searchable list of concept-tagged steps/notes across a topic (or across all topics).

## 5. Essential features beyond the core graph interaction

These are treated as first-class requirements, not stretch goals — the app isn't usable without at least §5.1 and §5.2, and the rest close gaps that would otherwise surface immediately in real use.

### 5.1 Auth
The app must support two ways to connect to the model, configurable from within the UI (e.g. a settings panel, not a config file):
- **Anthropic account login** (OAuth-style flow).
- **Raw API key entry**, stored securely at rest (see open question in §10 on storage mechanism — OS keychain is the likely default).
Users should be able to switch between the two and see which is active. No topic-creation or spine-generation action should be reachable before one of these is configured.

### 5.2 Smooth graph navigation
The mini-map/graph view (§4.4) is not just a nice-to-have overview — the interaction model for it needs to be genuinely smooth:
- Pan and zoom around the graph without jank, at the same performance bar as the linear spine view.
- Read a node's content directly from the graph view (e.g. hover/click to preview) without a jarring full-context-switch every time.
- **Maximize** any node (spine step or side note) into a full, focused reading view, then return to the graph without losing scroll position or zoom level.

### 5.3 Model / generation settings
Expose model choice and a "step size" control in settings, rather than hardcoding the brevity target from §6.1 into the prompt. Users should be able to loosen or tighten how small a "step" is.

### 5.4 Regenerate / edit a step
If a spine step misses the mark:
- **Regenerate** it, optionally with new steering text, replacing (or versioning — TBD) the existing content.
- **Hand-edit** the AI's text directly, since sometimes the fix is smaller than a full regeneration.
Either action should be reflected in the concept ledger (§6.3) if the step's content materially changes.

### 5.5 Search within a topic
Find a spine step or side note by content — not just by scrolling the spine or hunting through the graph view. Cross-topic search is a later extension (§8).

### 5.6 Topic/session management
Rename, delete, duplicate, and reorder topics in the library. Some lightweight status indicator (e.g. "in progress" vs "revisit later") so the library is usable once it has more than a handful of topics.

### 5.7 Persistence & backup
Autosave continuously as the user works, not just on close or app exit. Provide a path to back up and restore the local store (see §7 storage notes) so a corrupted file doesn't silently destroy a topic's history.

### 5.8 Error / offline handling
Since every spine step and side note depends on a live API call, failure states need explicit UI treatment: clear messaging for API errors, rate limits, and no-network conditions, plus a retry path. This should be designed as a first-class state for spine/side-note generation, not a generic toast/error banner bolted on afterward.

### 5.9 Usage / cost visibility
A running token or cost estimate, at minimum per topic. Particularly important for users on their own API key (§5.1), who are directly paying per call.

### 5.10 Keyboard-first navigation
Shortcuts for the most repeated actions: advancing the spine, opening a side note on selected text, and navigating the graph view — since the core interaction loop (read → highlight → ask) benefits from not reaching for the mouse each time.

### 5.11 Import / export
- **Export**: a topic (spine + resolved side notes as footnotes) to markdown — see also §9.
- **Import**: seed a new topic from pasted or imported external text (e.g. a paper abstract or existing notes), used as the starting context for the first spine step.

## 6. LLM context management

This is the part that determines whether the app actually feels like "small steps" or just becomes a normal chatbot with extra UI. Treat spine and side-note generation as two different prompting contexts.

### 6.1 Spine generation
Context sent to the model:
- Full (or truncated/summarized, once long) linear history of prior spine steps.
- The concept ledger (§6.3) — a compact list of concepts already covered, including ones resolved via side notes — so the model doesn't need full side-note transcripts to avoid re-explaining something.
- The user's steering text, if provided.

Context explicitly **not** sent: full side-note thread content. This is what keeps spine generation cheap and keeps the model's steps tightly scoped.

System-level instruction (paraphrase, tune in practice):
> Explain only the single next concept needed to build toward the topic. 2–3 short paragraphs maximum. Do not preview what comes next. Do not re-explain concepts already marked as covered in the concept ledger below.

### 6.2 Side note generation
Context sent to the model:
- The anchored text + enough surrounding spine-step content for grounding.
- The side note's own thread history (typically short).

Context explicitly **not** required: the rest of the spine history — side notes are meant to be cheap, scoped, and fast.

System-level instruction (paraphrase):
> Answer only the highlighted question concisely, in a conversational register. One short paragraph unless the user asks for more.

### 6.3 Concept ledger
A structured, app-maintained (not necessarily user-facing) list per topic: `{concept_label, source}` pairs, populated as spine steps and resolved side notes are created. Purpose:
- Fed into every spine-generation call to preserve continuity without replaying full transcripts.
- Prevents the model from re-teaching something already covered in a side note.
- Doubles as a lightweight review/study artifact later (e.g. "everything I've learned about ReSTIR so far").

Populating the ledger can itself be an LLM call (extract concept labels from a step/note) or a simpler heuristic in v1 — worth prototyping both.

## 7. Technical notes (Rust desktop)

Not the focus of this doc, but flagged for the build:
- UI framework candidates: `egui` (immediate-mode, simpler highlight/popover handling) vs `Tauri` (web-tech UI, easier rich text + custom highlighting via JS/CSS, likely better fit for the text-selection-to-popover interaction described in §4.3).
- Storage: one file (or embedded SQLite) per topic, or a single local SQLite database with topics/steps/side_notes as tables. Given the data model in §3 is fairly relational (side notes reference spine steps by id), SQLite is a natural fit even for a local-first app.
- Text anchoring resilience: since `TextAnchor` stores a quoted-text snapshot, anchor re-location on load can fall back to a text search within the step content if offsets drift (e.g. after a content migration), rather than relying solely on stored offsets.

## 8. MVP scope and sequencing

Build in this order:
1. Auth: Anthropic login or API key entry (§5.1) — nothing else works without this.
2. Linear spine chat with the `+` step button (unprompted and steered).
3. Text highlighting → side note creation, with auto-seeded first message.
4. Resolve/collapse behavior for side notes.
5. Concept ledger, wired into spine-generation context.
6. Smooth graph navigation and node maximize/read view (§5.2).
7. Persistence & backup (§5.7) and basic error/offline handling (§5.8).

Defer to post-MVP:
- Mini-map / graph visualization view.
- Multi-topic sidebar/library polish beyond basic rename/delete.
- Concept index/search across topics (single-topic search is MVP; cross-topic is not).
- Export (see §9).
- Regenerate/edit a step, model/generation settings, usage/cost visibility, keyboard-first navigation, import to seed a topic.
- Forking/branching the spine (a separate feature, not to be conflated with side notes).

## 9. Future ideas (not committed)

- **Recap/zoom-out**: on-demand summary of the last N spine steps into one recap block for long topics.
- **Concept tags**: auto-tag each spine step with the concept(s) it introduces, queryable across topics.
- **Pace/difficulty control**: persistent per-topic setting (e.g. "assume graphics background, not sampling theory") folded into the spine system prompt.
- **Export**: spine + resolved side notes as footnotes → clean markdown study document.

## 10. Open questions for implementation

- Exact truncation/summarization strategy once a spine exceeds N steps (full history vs rolling summary vs concept-ledger-only).
- Whether concept-ledger extraction should be a dedicated LLM call per step or a cheaper heuristic.
- Whether side notes should ever be promotable into the spine (e.g. "this clarification was actually important, add it as a real step") — deliberately excluded from v1 but worth deciding early whether the data model should anticipate it.
- Where API keys are stored at rest (OS keychain vs local encrypted file) — needs a decision before §5.1 is implemented.
- Whether cost/usage tracking (§5.9) is estimated client-side from token counts or read back from the API's own usage reporting, where available.
