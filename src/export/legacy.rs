use std::path::Path;

use rusqlite::{params, Connection};
use serde::Serialize;
use tempfile::NamedTempFile;
use tracing::info;

use crate::error::{RecallError, Result};

/// Colonnes requises par ExploGPT (`domain/database.py`).
pub const REQUIRED_COLUMNS: &[&str] = &[
    "conversation_id",
    "title",
    "role",
    "timestamp",
    "id",
    "content",
    "IC",
    "model_slug",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacyExportScope {
    /// Preserve the historical projection: every active message, including branches.
    AllActive,
    /// Export only the selected current thread for each conversation.
    CurrentThread,
}

impl LegacyExportScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AllActive => "all-active",
            Self::CurrentThread => "current-thread",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct LegacyExportStats {
    pub messages_exported: u64,
    pub conversations: u64,
    pub min_ic: i64,
    pub max_ic: i64,
    pub fts_available: bool,
    pub branch_scope: String,
}

pub fn export_legacy_sqlite(source: &Connection, output_path: &Path) -> Result<LegacyExportStats> {
    export_legacy_sqlite_with_scope(source, output_path, LegacyExportScope::AllActive)
}

pub fn export_legacy_sqlite_with_scope(
    source: &Connection,
    output_path: &Path,
    scope: LegacyExportScope,
) -> Result<LegacyExportStats> {
    if let Some(parent) = output_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Build and validate a sibling temporary database first. The old export is
    // replaced only after every integrity check succeeds.
    let temporary = NamedTempFile::new_in(output_path.parent().unwrap_or_else(|| Path::new(".")))?;
    let temporary_path = temporary.path().to_path_buf();
    let out = Connection::open(&temporary_path)?;
    out.execute_batch(
        "CREATE TABLE messages (
            conversation_id TEXT,
            title TEXT,
            role TEXT,
            timestamp TEXT,
            id TEXT PRIMARY KEY,
            content TEXT,
            IC INTEGER NOT NULL UNIQUE,
            token_count INTEGER,
            model_slug TEXT
        );
        CREATE TABLE export_metadata (
            key TEXT PRIMARY KEY,
            value TEXT NOT NULL
        );",
    )?;
    out.execute(
        "INSERT INTO export_metadata(key, value) VALUES ('branch_scope', ?1)",
        [scope.as_str()],
    )?;

    let source_query = match scope {
        LegacyExportScope::AllActive => "SELECT m.conversation_id,
                c.title,
                m.role,
                m.timestamp,
                m.id,
                m.ic,
                COALESCE(
                    (SELECT GROUP_CONCAT(COALESCE(cb.text_content, ''), char(10) ORDER BY cb.ordinal)
                     FROM content_blocks cb
                     WHERE cb.message_id = m.id),
                    ''
                ) AS content,
                m.model_slug
         FROM messages m
         JOIN conversations c ON c.id = m.conversation_id
         WHERE m.is_active = 1
         ORDER BY m.ic",
        LegacyExportScope::CurrentThread => "WITH RECURSIVE active_path(conversation_id, node_id) AS (
             SELECT id, current_node_id
             FROM conversations
             WHERE is_active = 1 AND current_node_id IS NOT NULL
             UNION ALL
             SELECT path.conversation_id, n.parent_id
             FROM active_path path
             JOIN nodes n ON n.id = path.node_id
             WHERE n.is_active = 1 AND n.parent_id IS NOT NULL
         )
         SELECT m.conversation_id,
                c.title,
                m.role,
                m.timestamp,
                m.id,
                m.ic,
                COALESCE(
                    (SELECT GROUP_CONCAT(COALESCE(cb.text_content, ''), char(10) ORDER BY cb.ordinal)
                     FROM content_blocks cb
                     WHERE cb.message_id = m.id),
                    ''
                ) AS content,
                m.model_slug
         FROM messages m
         JOIN conversations c ON c.id = m.conversation_id
         LEFT JOIN active_path path
           ON path.conversation_id = m.conversation_id AND path.node_id = m.node_id
         WHERE m.is_active = 1
           AND (c.current_node_id IS NULL OR path.node_id IS NOT NULL)
         ORDER BY m.ic",
    };
    let mut stmt = source.prepare(source_query)?;

    let rows = stmt.query_map([], |row| {
        Ok(LegacyMessageRow {
            conversation_id: row.get(0)?,
            title: row.get(1)?,
            role: row.get(2)?,
            timestamp: row.get(3)?,
            id: row.get(4)?,
            ic: row.get(5)?,
            content: row.get::<_, Option<String>>(6)?.unwrap_or_default(),
            model_slug: row.get(7)?,
        })
    })?;

    let mut messages_exported = 0u64;
    let mut conversations = std::collections::HashSet::new();
    let mut min_ic = i64::MAX;
    let mut max_ic = 0i64;

    for row in rows {
        let row = row?;
        conversations.insert(row.conversation_id.clone());
        min_ic = min_ic.min(row.ic);
        max_ic = max_ic.max(row.ic);
        let token_count = compute_token_count(&row.content);
        out.execute(
            "INSERT INTO messages
             (conversation_id, title, role, timestamp, id, content, IC, token_count, model_slug)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9)",
            params![
                row.conversation_id,
                row.title,
                row.role,
                row.timestamp,
                row.id,
                row.content,
                row.ic,
                token_count,
                row.model_slug,
            ],
        )?;
        messages_exported += 1;
    }

    if messages_exported == 0 {
        min_ic = 0;
    }

    validate_legacy_schema(&out)?;
    validate_export_integrity(source, &out, scope)?;
    create_titles_view(&out)?;
    let fts_available = ensure_legacy_fts5(&out)?;

    let stats = LegacyExportStats {
        messages_exported,
        conversations: conversations.len() as u64,
        min_ic: if min_ic == i64::MAX { 0 } else { min_ic },
        max_ic,
        fts_available,
        branch_scope: scope.as_str().to_string(),
    };

    drop(out);
    std::fs::rename(temporary.path(), output_path)?;

    info!(
        messages = stats.messages_exported,
        conversations = stats.conversations,
        min_ic = stats.min_ic,
        max_ic = stats.max_ic,
        fts_available = stats.fts_available,
        "legacy export written to {:?}",
        output_path
    );

    Ok(stats)
}

#[derive(Debug)]
struct LegacyMessageRow {
    conversation_id: String,
    title: Option<String>,
    role: Option<String>,
    timestamp: Option<String>,
    id: String,
    ic: i64,
    content: String,
    model_slug: Option<String>,
}

pub fn compute_token_count(text: &str) -> i64 {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return 0;
    }
    std::cmp::max(1, (trimmed.len() / 4) as i64)
}

pub fn validate_legacy_schema(conn: &Connection) -> Result<()> {
    let mut stmt = conn.prepare("PRAGMA table_info(messages)")?;
    let columns: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<std::result::Result<_, _>>()?;
    let column_set: std::collections::HashSet<_> = columns.iter().map(String::as_str).collect();

    for required in REQUIRED_COLUMNS {
        if !column_set.contains(required) {
            return Err(RecallError::msg(format!(
                "legacy export missing required column: {required}"
            )));
        }
    }
    Ok(())
}

pub fn validate_ic_preserved(source: &Connection, output: &Connection) -> Result<()> {
    validate_export_integrity(source, output, LegacyExportScope::AllActive)
}

pub fn validate_export_integrity(
    source: &Connection,
    output: &Connection,
    scope: LegacyExportScope,
) -> Result<()> {
    let active_count: i64 = source.query_row(&source_count_query(scope), [], |r| r.get(0))?;
    let exported_count: i64 =
        output.query_row("SELECT COUNT(*) FROM messages", [], |r| r.get(0))?;
    if active_count != exported_count {
        return Err(RecallError::msg(format!(
            "legacy export row count mismatch: source active={active_count}, exported={exported_count}"
        )));
    }

    let mismatches = {
        let mut stmt = source.prepare(&source_integrity_query(scope))?;
        let canonical = stmt
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;

        let mut ic_mismatches = 0i64;
        let mut model_mismatches = 0i64;
        for (id, ic, model_slug) in canonical {
            let legacy_ic: i64 =
                output.query_row("SELECT IC FROM messages WHERE id = ?1", [&id], |r| r.get(0))?;
            if legacy_ic != ic {
                ic_mismatches += 1;
            }
            let legacy_model: Option<String> = output.query_row(
                "SELECT model_slug FROM messages WHERE id = ?1",
                [&id],
                |r| r.get(0),
            )?;
            if legacy_model != model_slug {
                model_mismatches += 1;
            }
        }
        (ic_mismatches, model_mismatches)
    };

    if mismatches.0 > 0 {
        return Err(RecallError::msg(format!(
            "{} IC values differ between canonical and legacy export",
            mismatches.0
        )));
    }
    if mismatches.1 > 0 {
        return Err(RecallError::msg(format!(
            "{} model_slug values differ between canonical and legacy export",
            mismatches.1
        )));
    }

    Ok(())
}

fn source_count_query(scope: LegacyExportScope) -> String {
    match scope {
        LegacyExportScope::AllActive => {
            "SELECT COUNT(*) FROM messages WHERE is_active = 1".to_string()
        }
        LegacyExportScope::CurrentThread => {
            "WITH RECURSIVE active_path(conversation_id, node_id) AS (
                SELECT id, current_node_id FROM conversations
                WHERE is_active = 1 AND current_node_id IS NOT NULL
                UNION ALL
                SELECT path.conversation_id, n.parent_id
                FROM active_path path JOIN nodes n ON n.id = path.node_id
                WHERE n.is_active = 1 AND n.parent_id IS NOT NULL
            )
            SELECT COUNT(*)
            FROM messages m JOIN conversations c ON c.id = m.conversation_id
            LEFT JOIN active_path path
              ON path.conversation_id = m.conversation_id AND path.node_id = m.node_id
            WHERE m.is_active = 1
              AND (c.current_node_id IS NULL OR path.node_id IS NOT NULL)"
                .to_string()
        }
    }
}

fn source_integrity_query(scope: LegacyExportScope) -> String {
    match scope {
        LegacyExportScope::AllActive => {
            "SELECT m.id, m.ic, m.model_slug FROM messages m WHERE m.is_active = 1 ORDER BY m.ic"
                .to_string()
        }
        LegacyExportScope::CurrentThread => {
            "WITH RECURSIVE active_path(conversation_id, node_id) AS (
                SELECT id, current_node_id FROM conversations
                WHERE is_active = 1 AND current_node_id IS NOT NULL
                UNION ALL
                SELECT path.conversation_id, n.parent_id
                FROM active_path path JOIN nodes n ON n.id = path.node_id
                WHERE n.is_active = 1 AND n.parent_id IS NOT NULL
            )
            SELECT m.id, m.ic, m.model_slug
            FROM messages m JOIN conversations c ON c.id = m.conversation_id
            LEFT JOIN active_path path
              ON path.conversation_id = m.conversation_id AND path.node_id = m.node_id
            WHERE m.is_active = 1
              AND (c.current_node_id IS NULL OR path.node_id IS NOT NULL)
            ORDER BY m.ic"
                .to_string()
        }
    }
}

pub fn create_titles_view(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE VIEW IF NOT EXISTS titles AS
         SELECT m.conversation_id,
                COALESCE(
                  (
                    SELECT m2.title FROM messages m2
                    WHERE m2.conversation_id = m.conversation_id
                      AND m2.title IS NOT NULL AND m2.title != ''
                    ORDER BY
                      COALESCE(m2.timestamp, '9999') ASC,
                      COALESCE(m2.IC, 999999999) ASC,
                      m2.rowid ASC
                    LIMIT 1
                  ),
                  '(untitled)'
                ) AS title
         FROM messages m
         GROUP BY m.conversation_id;",
    )?;
    Ok(())
}

/// Aligns `messages_fts` and its triggers with the GPTExtractor / ExploGPT schema.
pub fn ensure_legacy_fts5(conn: &Connection) -> Result<bool> {
    let has_messages: i64 = conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='messages'",
        [],
        |r| r.get(0),
    )?;
    if has_messages == 0 {
        return Ok(false);
    }

    if conn
        .execute_batch(
            "CREATE VIRTUAL TABLE IF NOT EXISTS temp.__fts5_probe USING fts5(x);
             DROP TABLE IF EXISTS temp.__fts5_probe;",
        )
        .is_err()
    {
        return Ok(false);
    }

    conn.execute_batch(
        "DROP TRIGGER IF EXISTS messages_ai;
         DROP TRIGGER IF EXISTS messages_ad;
         DROP TRIGGER IF EXISTS messages_au;
         DROP TABLE IF EXISTS messages_fts;",
    )?;

    conn.execute_batch(
        "CREATE VIRTUAL TABLE messages_fts USING fts5(
            content,
            conversation_id UNINDEXED,
            role UNINDEXED,
            timestamp UNINDEXED,
            message_id UNINDEXED,
            content='messages',
            content_rowid='rowid',
            tokenize='unicode61 remove_diacritics 2'
         );",
    )?;

    conn.execute_batch(
        "CREATE TRIGGER messages_ai AFTER INSERT ON messages BEGIN
           INSERT INTO messages_fts(rowid, content, conversation_id, role, timestamp, message_id)
           VALUES (new.rowid, new.content, new.conversation_id, new.role, new.timestamp, new.id);
         END;

         CREATE TRIGGER messages_ad AFTER DELETE ON messages BEGIN
           INSERT INTO messages_fts(messages_fts, rowid, content)
           VALUES('delete', old.rowid, old.content);
         END;

         CREATE TRIGGER messages_au AFTER UPDATE ON messages BEGIN
           INSERT INTO messages_fts(messages_fts, rowid, content)
           VALUES('delete', old.rowid, old.content);
           INSERT INTO messages_fts(rowid, content, conversation_id, role, timestamp, message_id)
           VALUES (new.rowid, new.content, new.conversation_id, new.role, new.timestamp, new.id);
         END;

         INSERT INTO messages_fts(rowid, content, conversation_id, role, timestamp, message_id)
         SELECT rowid, content, conversation_id, role, timestamp, id
         FROM messages;",
    )?;

    Ok(true)
}

pub fn legacy_fts_available(conn: &Connection) -> bool {
    conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='messages_fts'",
        [],
        |r| r.get::<_, i64>(0),
    )
    .map(|count| count > 0)
    .unwrap_or(false)
}
