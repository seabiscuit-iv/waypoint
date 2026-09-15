//! SQLite persistence. One local database, autosaved on every mutation.

use std::collections::HashMap;

use rusqlite::{params, Connection, OptionalExtension, Row};
use uuid::Uuid;

use crate::markdown;
use crate::models::*;

pub fn open(path: &std::path::Path) -> rusqlite::Result<Connection> {
    let conn = Connection::open(path)?;
    let _: String = conn.query_row("PRAGMA journal_mode=WAL", [], |row| row.get(0))?;
    conn.execute_batch("PRAGMA foreign_keys=ON;")?;
    migrate(&conn)?;
    Ok(conn)
}

fn migrate(conn: &Connection) -> rusqlite::Result<()> {
    conn.execute_batch(
        r#"
        CREATE TABLE IF NOT EXISTS topics (
            id            TEXT PRIMARY KEY,
            title         TEXT NOT NULL,
            status        TEXT NOT NULL DEFAULT 'learning',
            position      INTEGER NOT NULL DEFAULT 0,
            seed_context  TEXT,
            input_tokens  INTEGER NOT NULL DEFAULT 0,
            output_tokens INTEGER NOT NULL DEFAULT 0,
            cost_usd      REAL NOT NULL DEFAULT 0,
            created_at    TEXT NOT NULL,
            updated_at    TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS spine_steps (
            id         TEXT PRIMARY KEY,
            topic_id   TEXT NOT NULL REFERENCES topics(id) ON DELETE CASCADE,
            position   INTEGER NOT NULL,
            prompt     TEXT,
            content    TEXT NOT NULL,
            created_at TEXT NOT NULL,
            updated_at TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS side_notes (
            id             TEXT PRIMARY KEY,
            topic_id       TEXT NOT NULL REFERENCES topics(id) ON DELETE CASCADE,
            anchor_step_id TEXT NOT NULL REFERENCES spine_steps(id) ON DELETE CASCADE,
            start_offset   INTEGER NOT NULL,
            end_offset     INTEGER NOT NULL,
            quoted_text    TEXT NOT NULL,
            resolved       INTEGER NOT NULL DEFAULT 0,
            created_at     TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS side_note_messages (
            id           TEXT PRIMARY KEY,
            side_note_id TEXT NOT NULL REFERENCES side_notes(id) ON DELETE CASCADE,
            position     INTEGER NOT NULL,
            role         TEXT NOT NULL,
            content      TEXT NOT NULL,
            created_at   TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS concept_ledger (
            id          TEXT PRIMARY KEY,
            topic_id    TEXT NOT NULL REFERENCES topics(id) ON DELETE CASCADE,
            label       TEXT NOT NULL,
            source_kind TEXT NOT NULL,
            source_id   TEXT NOT NULL,
            created_at  TEXT NOT NULL
        );

        CREATE TABLE IF NOT EXISTS settings (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_steps_topic  ON spine_steps(topic_id, position);
        CREATE INDEX IF NOT EXISTS idx_notes_topic  ON side_notes(topic_id);
        CREATE INDEX IF NOT EXISTS idx_notes_step   ON side_notes(anchor_step_id);
        CREATE INDEX IF NOT EXISTS idx_note_msgs    ON side_note_messages(side_note_id, position);
        CREATE INDEX IF NOT EXISTS idx_ledger_topic ON concept_ledger(topic_id);
        "#,
    )?;
    add_column_if_missing(conn, "topics", "prior_knowledge", "TEXT")
}

fn add_column_if_missing(
    conn: &Connection,
    table: &str,
    column: &str,
    decl: &str,
) -> rusqlite::Result<()> {
    let exists = conn
        .prepare(&format!("SELECT 1 FROM pragma_table_info('{table}') WHERE name = ?1"))?
        .exists(params![column])?;
    if !exists {
        conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {decl};"))?;
    }
    Ok(())
}

// ---------------------------------------------------------------- topics --

const SUMMARY_SQL: &str = "SELECT t.id, t.title, t.status, t.position, t.created_at, t.updated_at, \
    (SELECT COUNT(*) FROM spine_steps s WHERE s.topic_id = t.id), \
    (SELECT COUNT(*) FROM side_notes n WHERE n.topic_id = t.id AND n.resolved = 0), \
    t.input_tokens, t.output_tokens, t.cost_usd \
    FROM topics t";

fn summary_from_row(row: &Row) -> rusqlite::Result<TopicSummary> {
    Ok(TopicSummary {
        id: row.get(0)?,
        title: row.get(1)?,
        status: row.get(2)?,
        position: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
        step_count: row.get(6)?,
        open_note_count: row.get(7)?,
        input_tokens: row.get(8)?,
        output_tokens: row.get(9)?,
        cost_usd: row.get(10)?,
    })
}

pub fn list_topics(conn: &Connection) -> rusqlite::Result<Vec<TopicSummary>> {
    let sql = format!("{SUMMARY_SQL} ORDER BY t.position ASC, t.created_at ASC");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map([], summary_from_row)?;
    rows.collect()
}

pub fn get_topic_summary(conn: &Connection, id: &str) -> rusqlite::Result<TopicSummary> {
    let sql = format!("{SUMMARY_SQL} WHERE t.id = ?1");
    conn.query_row(&sql, params![id], summary_from_row)
}

pub fn get_topic_row(conn: &Connection, id: &str) -> rusqlite::Result<TopicRow> {
    conn.query_row(
        "SELECT id, title, status, seed_context, prior_knowledge, created_at FROM topics WHERE id = ?1",
        params![id],
        |r| {
            Ok(TopicRow {
                id: r.get(0)?,
                title: r.get(1)?,
                status: r.get(2)?,
                seed_context: r.get(3)?,
                prior_knowledge: r.get(4)?,
                created_at: r.get(5)?,
            })
        },
    )
}

pub fn create_topic(
    conn: &Connection,
    id: &str,
    title: &str,
    seed_context: Option<&str>,
    prior_knowledge: Option<&str>,
    now: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO topics (id, title, status, position, seed_context, prior_knowledge, created_at, updated_at) \
         VALUES (?1, ?2, 'learning', COALESCE((SELECT MAX(position) + 1 FROM topics), 0), ?3, ?4, ?5, ?5)",
        params![id, title, seed_context, prior_knowledge, now],
    )?;
    Ok(())
}

pub fn rename_topic(conn: &Connection, id: &str, title: &str, now: &str) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE topics SET title = ?2, updated_at = ?3 WHERE id = ?1",
        params![id, title, now],
    )?;
    Ok(())
}

pub fn set_topic_status(
    conn: &Connection,
    id: &str,
    status: &str,
    now: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE topics SET status = ?2, updated_at = ?3 WHERE id = ?1",
        params![id, status, now],
    )?;
    Ok(())
}

pub fn delete_topic(conn: &Connection, id: &str) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM topics WHERE id = ?1", params![id])?;
    Ok(())
}

pub fn reorder_topics(conn: &Connection, ids: &[String]) -> rusqlite::Result<()> {
    let mut stmt = conn.prepare("UPDATE topics SET position = ?2 WHERE id = ?1")?;
    for (i, id) in ids.iter().enumerate() {
        stmt.execute(params![id, i as i64])?;
    }
    Ok(())
}

pub fn get_topic_detail(conn: &Connection, id: &str) -> rusqlite::Result<TopicDetail> {
    let row = get_topic_row(conn, id)?;
    Ok(TopicDetail {
        steps: get_steps(conn, id)?,
        side_notes: get_notes(conn, id)?,
        ledger: get_ledger(conn, id)?,
        usage: get_usage(conn, id)?,
        id: row.id,
        title: row.title,
        status: row.status,
        seed_context: row.seed_context,
        prior_knowledge: row.prior_knowledge,
        created_at: row.created_at,
    })
}

/// Deep-copies a topic (steps, notes, messages, ledger) under fresh ids.
/// Usage counters reset — duplication doesn't spend anything.
pub fn duplicate_topic(
    conn: &mut Connection,
    src_id: &str,
    now: &str,
) -> rusqlite::Result<String> {
    let tx = conn.transaction()?;
    let new_topic_id = Uuid::new_v4().to_string();

    let (title, status, seed, prior): (String, String, Option<String>, Option<String>) = tx.query_row(
        "SELECT title, status, seed_context, prior_knowledge FROM topics WHERE id = ?1",
        params![src_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )?;
    tx.execute(
        "INSERT INTO topics (id, title, status, position, seed_context, prior_knowledge, created_at, updated_at) \
         VALUES (?1, ?2, ?3, COALESCE((SELECT MAX(position) + 1 FROM topics), 0), ?4, ?5, ?6, ?6)",
        params![new_topic_id, format!("{title} (copy)"), status, seed, prior, now],
    )?;

    // Steps.
    let mut step_map: HashMap<String, String> = HashMap::new();
    let steps: Vec<(String, i64, Option<String>, String, String, String)> = {
        let mut stmt = tx.prepare(
            "SELECT id, position, prompt, content, created_at, updated_at \
             FROM spine_steps WHERE topic_id = ?1 ORDER BY position ASC",
        )?;
        let rows = stmt.query_map(params![src_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (old_id, position, prompt, content, created_at, updated_at) in &steps {
        let new_id = Uuid::new_v4().to_string();
        step_map.insert(old_id.clone(), new_id.clone());
        tx.execute(
            "INSERT INTO spine_steps (id, topic_id, position, prompt, content, created_at, updated_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![new_id, new_topic_id, position, prompt, content, created_at, updated_at],
        )?;
    }

    // Side notes.
    let mut note_map: HashMap<String, String> = HashMap::new();
    let notes: Vec<(String, String, i64, i64, String, i64, String)> = {
        let mut stmt = tx.prepare(
            "SELECT id, anchor_step_id, start_offset, end_offset, quoted_text, resolved, created_at \
             FROM side_notes WHERE topic_id = ?1",
        )?;
        let rows = stmt.query_map(params![src_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (old_id, anchor, start, end, quoted, resolved, created_at) in &notes {
        let Some(new_anchor) = step_map.get(anchor) else {
            continue;
        };
        let new_id = Uuid::new_v4().to_string();
        note_map.insert(old_id.clone(), new_id.clone());
        tx.execute(
            "INSERT INTO side_notes (id, topic_id, anchor_step_id, start_offset, end_offset, quoted_text, resolved, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![new_id, new_topic_id, new_anchor, start, end, quoted, resolved, created_at],
        )?;
    }

    // Note messages.
    let msgs: Vec<(String, i64, String, String, String)> = {
        let mut stmt = tx.prepare(
            "SELECT m.side_note_id, m.position, m.role, m.content, m.created_at \
             FROM side_note_messages m JOIN side_notes n ON n.id = m.side_note_id \
             WHERE n.topic_id = ?1",
        )?;
        let rows = stmt.query_map(params![src_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (old_note_id, position, role, content, created_at) in &msgs {
        let Some(new_note_id) = note_map.get(old_note_id) else {
            continue;
        };
        tx.execute(
            "INSERT INTO side_note_messages (id, side_note_id, position, role, content, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![Uuid::new_v4().to_string(), new_note_id, position, role, content, created_at],
        )?;
    }

    // Concept ledger.
    let ledger: Vec<(String, String, String, String)> = {
        let mut stmt = tx.prepare(
            "SELECT label, source_kind, source_id, created_at FROM concept_ledger WHERE topic_id = ?1",
        )?;
        let rows = stmt.query_map(params![src_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
        })?;
        rows.collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (label, kind, source_id, created_at) in &ledger {
        let remapped = match kind.as_str() {
            "spine" => step_map.get(source_id).cloned(),
            "side_note" => note_map.get(source_id).cloned(),
            _ => None,
        }
        .unwrap_or_else(|| source_id.clone());
        tx.execute(
            "INSERT INTO concept_ledger (id, topic_id, label, source_kind, source_id, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            params![Uuid::new_v4().to_string(), new_topic_id, label, kind, remapped, created_at],
        )?;
    }

    tx.commit()?;
    Ok(new_topic_id)
}

// ----------------------------------------------------------------- steps --

const STEP_COLS: &str = "id, topic_id, position, prompt, content, created_at, updated_at";

fn step_from_row(row: &Row) -> rusqlite::Result<SpineStep> {
    let content: String = row.get(4)?;
    Ok(SpineStep {
        id: row.get(0)?,
        topic_id: row.get(1)?,
        position: row.get(2)?,
        prompt: row.get(3)?,
        html: markdown::render(&content),
        content,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
    })
}

pub fn get_steps(conn: &Connection, topic_id: &str) -> rusqlite::Result<Vec<SpineStep>> {
    let sql = format!("SELECT {STEP_COLS} FROM spine_steps WHERE topic_id = ?1 ORDER BY position ASC");
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt.query_map(params![topic_id], step_from_row)?;
    rows.collect()
}

pub fn get_step(conn: &Connection, step_id: &str) -> rusqlite::Result<SpineStep> {
    let sql = format!("SELECT {STEP_COLS} FROM spine_steps WHERE id = ?1");
    conn.query_row(&sql, params![step_id], step_from_row)
}

pub fn next_step_position(conn: &Connection, topic_id: &str) -> rusqlite::Result<i64> {
    conn.query_row(
        "SELECT COALESCE(MAX(position) + 1, 0) FROM spine_steps WHERE topic_id = ?1",
        params![topic_id],
        |r| r.get(0),
    )
}

pub fn insert_step(
    conn: &Connection,
    id: &str,
    topic_id: &str,
    position: i64,
    prompt: Option<&str>,
    content: &str,
    now: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO spine_steps (id, topic_id, position, prompt, content, created_at, updated_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?6)",
        params![id, topic_id, position, prompt, content, now],
    )?;
    conn.execute(
        "UPDATE topics SET updated_at = ?2 WHERE id = ?1",
        params![topic_id, now],
    )?;
    Ok(())
}

/// Used by regeneration: keeps the step's id/position, replaces its content.
pub fn replace_step(
    conn: &Connection,
    step_id: &str,
    prompt: Option<&str>,
    content: &str,
    now: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE spine_steps SET prompt = ?2, content = ?3, updated_at = ?4 WHERE id = ?1",
        params![step_id, prompt, content, now],
    )?;
    Ok(())
}

pub fn update_step_content(
    conn: &Connection,
    step_id: &str,
    content: &str,
    now: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE spine_steps SET content = ?2, updated_at = ?3 WHERE id = ?1",
        params![step_id, content, now],
    )?;
    conn.execute(
        "UPDATE topics SET updated_at = ?2 WHERE id = (SELECT topic_id FROM spine_steps WHERE id = ?1)",
        params![step_id, now],
    )?;
    Ok(())
}

// ------------------------------------------------------------ side notes --

fn note_from_row(row: &Row) -> rusqlite::Result<SideNote> {
    Ok(SideNote {
        id: row.get(0)?,
        topic_id: row.get(1)?,
        anchor_step_id: row.get(2)?,
        start_offset: row.get(3)?,
        end_offset: row.get(4)?,
        quoted_text: row.get(5)?,
        resolved: row.get::<_, i64>(6)? != 0,
        created_at: row.get(7)?,
        messages: Vec::new(),
    })
}

fn note_message_from_row(row: &Row) -> rusqlite::Result<SideNoteMessage> {
    let content: String = row.get(3)?;
    Ok(SideNoteMessage {
        id: row.get(0)?,
        position: row.get(1)?,
        role: row.get(2)?,
        html: markdown::render(&content),
        content,
        created_at: row.get(4)?,
    })
}

pub fn get_notes(conn: &Connection, topic_id: &str) -> rusqlite::Result<Vec<SideNote>> {
    let mut msg_map: HashMap<String, Vec<SideNoteMessage>> = HashMap::new();
    {
        let mut stmt = conn.prepare(
            "SELECT m.id, m.position, m.role, m.content, m.created_at, m.side_note_id \
             FROM side_note_messages m JOIN side_notes n ON n.id = m.side_note_id \
             WHERE n.topic_id = ?1 ORDER BY m.side_note_id, m.position",
        )?;
        let mut rows = stmt.query(params![topic_id])?;
        while let Some(row) = rows.next()? {
            let note_id: String = row.get(5)?;
            msg_map
                .entry(note_id)
                .or_default()
                .push(note_message_from_row(row)?);
        }
    }

    let mut stmt = conn.prepare(
        "SELECT id, topic_id, anchor_step_id, start_offset, end_offset, quoted_text, resolved, created_at \
         FROM side_notes WHERE topic_id = ?1 ORDER BY created_at ASC",
    )?;
    let rows = stmt.query_map(params![topic_id], note_from_row)?;
    let mut notes: Vec<SideNote> = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    for note in &mut notes {
        note.messages = msg_map.remove(&note.id).unwrap_or_default();
    }
    Ok(notes)
}

pub fn get_note(conn: &Connection, note_id: &str) -> rusqlite::Result<SideNote> {
    let mut note = conn.query_row(
        "SELECT id, topic_id, anchor_step_id, start_offset, end_offset, quoted_text, resolved, created_at \
         FROM side_notes WHERE id = ?1",
        params![note_id],
        note_from_row,
    )?;
    let mut stmt = conn.prepare(
        "SELECT id, position, role, content, created_at FROM side_note_messages \
         WHERE side_note_id = ?1 ORDER BY position ASC",
    )?;
    let rows = stmt.query_map(params![note_id], note_message_from_row)?;
    note.messages = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(note)
}

pub fn insert_side_note(
    conn: &Connection,
    id: &str,
    topic_id: &str,
    anchor_step_id: &str,
    start_offset: i64,
    end_offset: i64,
    quoted_text: &str,
    now: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO side_notes (id, topic_id, anchor_step_id, start_offset, end_offset, quoted_text, resolved, created_at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 0, ?7)",
        params![id, topic_id, anchor_step_id, start_offset, end_offset, quoted_text, now],
    )?;
    Ok(())
}

pub fn insert_note_message(
    conn: &Connection,
    id: &str,
    note_id: &str,
    role: &str,
    content: &str,
    now: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "INSERT INTO side_note_messages (id, side_note_id, position, role, content, created_at) \
         VALUES (?1, ?2, COALESCE((SELECT MAX(position) + 1 FROM side_note_messages WHERE side_note_id = ?2), 0), ?3, ?4, ?5)",
        params![id, note_id, role, content, now],
    )?;
    Ok(())
}

pub fn get_note_message(conn: &Connection, id: &str) -> rusqlite::Result<SideNoteMessage> {
    conn.query_row(
        "SELECT id, position, role, content, created_at FROM side_note_messages WHERE id = ?1",
        params![id],
        note_message_from_row,
    )
}

pub fn set_note_resolved(conn: &Connection, note_id: &str, resolved: bool) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE side_notes SET resolved = ?2 WHERE id = ?1",
        params![note_id, resolved as i64],
    )?;
    Ok(())
}

pub fn delete_note(conn: &Connection, note_id: &str) -> rusqlite::Result<()> {
    conn.execute("DELETE FROM side_notes WHERE id = ?1", params![note_id])?;
    Ok(())
}

pub fn update_note_anchor(
    conn: &Connection,
    note_id: &str,
    start_offset: i64,
    end_offset: i64,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE side_notes SET start_offset = ?2, end_offset = ?3 WHERE id = ?1",
        params![note_id, start_offset, end_offset],
    )?;
    Ok(())
}

// -------------------------------------------------------- concept ledger --

pub fn ledger_labels(conn: &Connection, topic_id: &str) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT label FROM concept_ledger WHERE topic_id = ?1 ORDER BY created_at ASC",
    )?;
    let rows = stmt.query_map(params![topic_id], |r| r.get(0))?;
    rows.collect()
}

pub fn ledger_labels_excluding(
    conn: &Connection,
    topic_id: &str,
    source_id: &str,
) -> rusqlite::Result<Vec<String>> {
    let mut stmt = conn.prepare(
        "SELECT label FROM concept_ledger WHERE topic_id = ?1 AND source_id != ?2 ORDER BY created_at ASC",
    )?;
    let rows = stmt.query_map(params![topic_id, source_id], |r| r.get(0))?;
    rows.collect()
}

pub fn get_ledger(conn: &Connection, topic_id: &str) -> rusqlite::Result<Vec<ConceptEntry>> {
    let mut stmt = conn.prepare(
        "SELECT id, topic_id, label, source_kind, source_id, created_at \
         FROM concept_ledger WHERE topic_id = ?1 ORDER BY created_at ASC",
    )?;
    let rows = stmt.query_map(params![topic_id], |r| {
        Ok(ConceptEntry {
            id: r.get(0)?,
            topic_id: r.get(1)?,
            label: r.get(2)?,
            source_kind: r.get(3)?,
            source_id: r.get(4)?,
            created_at: r.get(5)?,
        })
    })?;
    rows.collect()
}

pub fn insert_ledger_entries(
    conn: &Connection,
    topic_id: &str,
    labels: &[String],
    source_kind: &str,
    source_id: &str,
    now: &str,
) -> rusqlite::Result<()> {
    for label in labels {
        let exists: i64 = conn.query_row(
            "SELECT COUNT(*) FROM concept_ledger WHERE topic_id = ?1 AND lower(label) = lower(?2)",
            params![topic_id, label],
            |r| r.get(0),
        )?;
        if exists == 0 {
            conn.execute(
                "INSERT INTO concept_ledger (id, topic_id, label, source_kind, source_id, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![Uuid::new_v4().to_string(), topic_id, label, source_kind, source_id, now],
            )?;
        }
    }
    Ok(())
}

pub fn delete_ledger_for_source(conn: &Connection, source_id: &str) -> rusqlite::Result<()> {
    conn.execute(
        "DELETE FROM concept_ledger WHERE source_id = ?1",
        params![source_id],
    )?;
    Ok(())
}

// ------------------------------------------------------------ usage/cost --

pub fn add_usage(
    conn: &Connection,
    topic_id: &str,
    input_tokens: i64,
    output_tokens: i64,
    cost_usd: f64,
    now: &str,
) -> rusqlite::Result<()> {
    conn.execute(
        "UPDATE topics SET input_tokens = input_tokens + ?2, output_tokens = output_tokens + ?3, \
         cost_usd = cost_usd + ?4, updated_at = ?5 WHERE id = ?1",
        params![topic_id, input_tokens, output_tokens, cost_usd, now],
    )?;
    Ok(())
}

pub fn get_usage(conn: &Connection, topic_id: &str) -> rusqlite::Result<UsageTotals> {
    conn.query_row(
        "SELECT input_tokens, output_tokens, cost_usd FROM topics WHERE id = ?1",
        params![topic_id],
        |r| {
            Ok(UsageTotals {
                input_tokens: r.get(0)?,
                output_tokens: r.get(1)?,
                cost_usd: r.get(2)?,
            })
        },
    )
}

// -------------------------------------------------------------- settings --

pub fn get_settings(conn: &Connection) -> rusqlite::Result<Settings> {
    let raw: Option<String> = conn
        .query_row("SELECT value FROM settings WHERE key = 'app'", [], |r| r.get(0))
        .optional()?;
    Ok(raw
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default())
}

pub fn set_settings(conn: &Connection, settings: &Settings) -> rusqlite::Result<()> {
    let json = serde_json::to_string(settings).unwrap_or_default();
    conn.execute(
        "INSERT INTO settings (key, value) VALUES ('app', ?1) \
         ON CONFLICT(key) DO UPDATE SET value = ?1",
        params![json],
    )?;
    Ok(())
}
