use std::path::Path;

use rusqlite::{params, Connection, OpenFlags, OptionalExtension};
use serde::{de::DeserializeOwned, Deserialize, Serialize};

use crate::domain::reference::{
    get_active_message_by_ic, get_active_message_by_id, get_ic_context, is_reference_role,
    resolve_message_reference, ContextScope, IcContext, MessageReference, ReferencedMessage,
};
use crate::error::{RecallError, Result};
use crate::storage::TRUSTED_REFERENCE_ROLE_PREDICATE;

// Trusted static predicate fragment: never accept runtime input here.

#[derive(Debug, Clone, PartialEq)]
pub struct ConversationListItem {
    pub id: String,
    pub title: String,
    pub updated_at: Option<f64>,
    pub message_count: i64,
    pub excerpt: String,
    pub default_model_slug: Option<String>,
    pub has_branches: bool,
    pub sort_time: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConversationMeta {
    pub id: String,
    pub title: String,
    pub current_node_id: Option<String>,
    pub updated_at: Option<f64>,
    pub default_model_slug: Option<String>,
    pub message_count: i64,
    pub has_branches: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IcJumpTarget {
    pub message_id: String,
    pub conversation_id: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MessageView {
    pub id: String,
    pub ic: Option<i64>,
    pub node_id: String,
    pub parent_node_id: Option<String>,
    pub conversation_id: String,
    pub role: String,
    pub model_slug: Option<String>,
    pub content: String,
    pub created_at: Option<f64>,
    pub timestamp: Option<String>,
}

impl MessageView {
    pub fn is_technical(&self) -> bool {
        !is_reference_role(Some(&self.role))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SearchHit {
    pub conversation_id: String,
    pub conversation_title: String,
    pub message_id: String,
    pub ic: Option<i64>,
    pub role: String,
    pub model_slug: Option<String>,
    pub node_id: String,
    pub parent_node_id: Option<String>,
    pub excerpt: String,
    pub created_at: Option<f64>,
    pub timestamp: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BranchChoice {
    pub node_id: String,
    pub message_id: Option<String>,
    pub ic: Option<i64>,
    pub role: Option<String>,
    pub preview: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AssetView {
    pub id: String,
    pub name: String,
    pub mime_type: Option<String>,
    pub exists_locally: bool,
    pub relative_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GraphNodeView {
    pub id: String,
    pub parent_id: Option<String>,
    pub message_id: Option<String>,
    pub ic: Option<i64>,
    pub role: Option<String>,
    pub preview: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AssetListItem {
    pub id: String,
    pub name: String,
    pub mime_type: Option<String>,
    pub exists_locally: bool,
    pub conversation_id: String,
    pub conversation_title: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MessageViewMode {
    CurrentThread,
    AllActive,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ReadPage<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MessageWindow {
    pub center: MessageView,
    pub before: Vec<MessageView>,
    pub after: Vec<MessageView>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ConversationCursor {
    sort_time: f64,
    id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct MessageCursor {
    view: String,
    ic: Option<i64>,
    depth: Option<i64>,
    node_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SearchCursor {
    created_at: f64,
    message_id: String,
}

pub struct ReadRepository {
    conn: Connection,
}

impl ReadRepository {
    pub fn open_read_only(path: &Path) -> Result<Self> {
        if !path.is_file() {
            return Err(RecallError::msg("Database not found"));
        }
        crate::semantic::store::ensure_sqlite_vec_registered();
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        let repository = Self { conn };
        repository.validate_schema()?;
        Ok(repository)
    }

    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    fn validate_schema(&self) -> Result<()> {
        for table in [
            "import_runs",
            "conversations",
            "nodes",
            "messages",
            "content_blocks",
            "conversations_fts",
            "recall_engine_migrations",
        ] {
            let exists: i64 = self.conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
                [table],
                |row| row.get(0),
            )?;
            if exists == 0 {
                return Err(RecallError::msg("Unsupported RecallEngine database schema"));
            }
        }
        let columns = self
            .conn
            .prepare("PRAGMA table_info(messages)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if !columns.iter().any(|column| column == "ic")
            || !columns.iter().any(|column| column == "model_slug")
        {
            return Err(RecallError::msg("Unsupported RecallEngine database schema"));
        }
        Ok(())
    }

    pub fn list_conversations(
        &self,
        term: &str,
        limit: usize,
    ) -> Result<Vec<ConversationListItem>> {
        self.list_conversations_page(term, limit, 0)
    }

    pub fn count_active_conversations(&self) -> Result<i64> {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM conversations WHERE is_active = 1",
                [],
                |row| row.get(0),
            )
            .map_err(Into::into)
    }

    pub fn list_conversations_page(
        &self,
        term: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<ConversationListItem>> {
        let term = term.trim();
        let limit = i64::try_from(limit.min(500)).unwrap_or(500);
        let offset = i64::try_from(offset).unwrap_or(i64::MAX);
        let pattern = format!("%{term}%");
        let fts_query = sanitize_fts_query(term);
        let mut statement = self.conn.prepare(
            "SELECT c.id, COALESCE(c.title, 'Untitled'), c.update_time,
                    COUNT(m.id),
                    COALESCE((SELECT cb.text_content
                      FROM messages lm JOIN content_blocks cb ON cb.message_id = lm.id
                      WHERE lm.conversation_id = c.id AND lm.is_active = 1 AND cb.text_content IS NOT NULL
                      ORDER BY COALESCE(lm.create_time, 0) DESC, cb.ordinal LIMIT 1), ''),
                    EXISTS(
                      SELECT 1 FROM nodes branch_nodes
                      WHERE branch_nodes.conversation_id = c.id AND branch_nodes.is_active = 1
                        AND branch_nodes.parent_id IS NOT NULL
                      GROUP BY branch_nodes.parent_id HAVING COUNT(*) > 1
                    ),
                    c.default_model_slug,
                    COALESCE(c.update_time, c.create_time, 0)
             FROM conversations c LEFT JOIN messages m ON m.conversation_id = c.id AND m.is_active = 1
             WHERE c.is_active = 1 AND (?1 = '' OR c.title LIKE ?2
               OR (?4 <> '' AND EXISTS (SELECT 1 FROM conversations_fts fts WHERE fts.rowid = c.rowid AND fts.conversations_fts MATCH ?4))
               OR EXISTS (
                 SELECT 1 FROM messages sm JOIN content_blocks sb ON sb.message_id = sm.id
                 WHERE sm.conversation_id = c.id AND sm.is_active = 1 AND sb.text_content LIKE ?2
               ))
             GROUP BY c.id ORDER BY COALESCE(c.update_time, c.create_time) DESC, c.id ASC LIMIT ?3 OFFSET ?5",
        )?;
        let rows =
            statement.query_map(params![term, pattern, limit, fts_query, offset], |row| {
                Ok(ConversationListItem {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    updated_at: row.get(2)?,
                    message_count: row.get(3)?,
                    excerpt: row.get(4)?,
                    has_branches: row.get::<_, i64>(5)? != 0,
                    default_model_slug: row.get(6)?,
                    sort_time: row.get(7)?,
                })
            })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn conversation_list_index(&self, conversation_id: &str) -> Result<Option<usize>> {
        let index: Option<i64> = self
            .conn
            .query_row(
                "SELECT COUNT(*)
                 FROM conversations c
                 JOIN conversations target ON target.id = ?1 AND target.is_active = 1
                 WHERE c.is_active = 1
                   AND (
                     COALESCE(c.update_time, c.create_time)
                       > COALESCE(target.update_time, target.create_time)
                     OR (
                       COALESCE(c.update_time, c.create_time)
                         = COALESCE(target.update_time, target.create_time)
                       AND c.id < target.id
                     )
                   )",
                [conversation_id],
                |row| row.get(0),
            )
            .optional()?;
        Ok(index.and_then(|value| usize::try_from(value).ok()))
    }

    pub fn conversation_meta(&self, id: &str) -> Result<Option<ConversationMeta>> {
        self.conn
            .query_row(
                "SELECT c.id, COALESCE(c.title, 'Untitled'), c.current_node_id,
                        c.update_time, c.default_model_slug, COUNT(m.id),
                        EXISTS(
                          SELECT 1 FROM nodes branch_nodes
                          WHERE branch_nodes.conversation_id = c.id
                            AND branch_nodes.is_active = 1
                            AND branch_nodes.parent_id IS NOT NULL
                          GROUP BY branch_nodes.parent_id HAVING COUNT(*) > 1
                        )
                 FROM conversations c
                 LEFT JOIN messages m ON m.conversation_id = c.id AND m.is_active = 1
                 WHERE c.id = ?1 AND c.is_active = 1
                 GROUP BY c.id",
                [id],
                |row| {
                    Ok(ConversationMeta {
                        id: row.get(0)?,
                        title: row.get(1)?,
                        current_node_id: row.get(2)?,
                        updated_at: row.get(3)?,
                        default_model_slug: row.get(4)?,
                        message_count: row.get(5)?,
                        has_branches: row.get::<_, i64>(6)? != 0,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn current_thread(&self, conversation_id: &str) -> Result<Vec<MessageView>> {
        let Some(meta) = self.conversation_meta(conversation_id)? else {
            return Ok(Vec::new());
        };
        let Some(node_id) = meta.current_node_id else {
            return self.all_messages(conversation_id, 500);
        };
        self.thread_for_node(&node_id)
    }

    pub fn thread_for_message(&self, message_id: &str) -> Result<Vec<MessageView>> {
        let node_id: Option<String> = self
            .conn
            .query_row(
                "SELECT node_id FROM messages WHERE id = ?1 AND is_active = 1",
                [message_id],
                |row| row.get(0),
            )
            .optional()?;
        node_id.map_or_else(|| Ok(Vec::new()), |node_id| self.thread_for_node(&node_id))
    }

    pub fn thread_for_node(&self, node_id: &str) -> Result<Vec<MessageView>> {
        let mut statement = self.conn.prepare(&format!(
            "WITH RECURSIVE path(id, parent_id, depth) AS (
                 SELECT id, parent_id, 0 FROM nodes WHERE id = ?1 AND is_active = 1
                 UNION ALL
                 SELECT n.id, n.parent_id, path.depth + 1
                 FROM nodes n JOIN path ON path.parent_id = n.id
                 WHERE n.is_active = 1
             )
             SELECT m.id,
                    CASE WHEN {TRUSTED_REFERENCE_ROLE_PREDICATE} THEN m.ic ELSE NULL END,
                    n.id, n.parent_id, m.conversation_id, COALESCE(m.role, 'unknown'), m.model_slug,
                    COALESCE((SELECT GROUP_CONCAT(text_content, char(10)) FROM (
                        SELECT cb.text_content FROM content_blocks cb
                        WHERE cb.message_id = m.id ORDER BY cb.ordinal
                    )), ''),
                    m.create_time, m.timestamp
             FROM path p JOIN nodes n ON n.id = p.id
             JOIN messages m ON m.node_id = n.id AND m.is_active = 1
             ORDER BY p.depth DESC"
        ))?;
        let rows = statement.query_map([node_id], map_message_view)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn all_messages(&self, conversation_id: &str, limit: usize) -> Result<Vec<MessageView>> {
        let limit = i64::try_from(limit.min(500)).unwrap_or(500);
        let mut statement = self.conn.prepare(&format!(
            "SELECT m.id,
                    CASE WHEN {TRUSTED_REFERENCE_ROLE_PREDICATE} THEN m.ic ELSE NULL END,
                    n.id, n.parent_id, m.conversation_id, COALESCE(m.role, 'unknown'), m.model_slug,
                    COALESCE((SELECT GROUP_CONCAT(text_content, char(10)) FROM (
                        SELECT cb.text_content FROM content_blocks cb
                        WHERE cb.message_id = m.id ORDER BY cb.ordinal
                    )), ''),
                    m.create_time, m.timestamp
             FROM messages m JOIN nodes n ON n.id = m.node_id
             WHERE m.conversation_id = ?1 AND m.is_active = 1 AND n.is_active = 1
             ORDER BY m.ic ASC LIMIT ?2"
        ))?;
        let rows = statement.query_map(params![conversation_id, limit], map_message_view)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn conversation_messages(&self, conversation_id: &str) -> Result<Vec<MessageView>> {
        let mut statement = self.conn.prepare(&format!(
            "SELECT m.id,
                    CASE WHEN {TRUSTED_REFERENCE_ROLE_PREDICATE} THEN m.ic ELSE NULL END,
                    n.id, n.parent_id, m.conversation_id, COALESCE(m.role, 'unknown'), m.model_slug,
                    COALESCE((SELECT GROUP_CONCAT(text_content, char(10)) FROM (
                        SELECT cb.text_content FROM content_blocks cb
                        WHERE cb.message_id = m.id ORDER BY cb.ordinal
                    )), ''),
                    m.create_time, m.timestamp
             FROM messages m JOIN nodes n ON n.id = m.node_id
             WHERE m.conversation_id = ?1 AND m.is_active = 1 AND n.is_active = 1
             ORDER BY COALESCE(m.create_time, 0), m.source_node_order"
        ))?;
        let rows = statement.query_map([conversation_id], map_message_view)?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn message_page(
        &self,
        conversation_id: &str,
        mode: MessageViewMode,
        limit: usize,
        cursor: Option<&str>,
    ) -> Result<ReadPage<MessageView>> {
        let cursor = decode_cursor::<MessageCursor>(cursor)?;
        if let Some(cursor) = &cursor {
            if cursor.view != mode.as_str() {
                return Err(RecallError::msg(
                    "invalid cursor for requested message view",
                ));
            }
        }
        let page_limit = limit.min(500);
        if page_limit == 0 {
            return Ok(ReadPage {
                items: Vec::new(),
                next_cursor: None,
            });
        }
        let query_limit = i64::try_from(page_limit + 1).unwrap_or(501);
        match mode {
            MessageViewMode::AllActive => {
                let cursor_ic = cursor.as_ref().and_then(|value| value.ic);
                let mut statement = self.conn.prepare(&format!(
                    "SELECT m.id,
                            CASE WHEN {TRUSTED_REFERENCE_ROLE_PREDICATE} THEN m.ic ELSE NULL END,
                            n.id, n.parent_id, m.conversation_id, COALESCE(m.role, 'unknown'), m.model_slug,
                            COALESCE((SELECT GROUP_CONCAT(text_content, char(10)) FROM (
                                SELECT cb.text_content FROM content_blocks cb
                                WHERE cb.message_id = m.id ORDER BY cb.ordinal
                            )), ''),
                            m.create_time, m.timestamp, m.ic
                     FROM messages m JOIN nodes n ON n.id = m.node_id
                     WHERE m.conversation_id = ?1 AND m.is_active = 1 AND n.is_active = 1
                       AND (?2 IS NULL OR m.ic > ?2)
                     ORDER BY m.ic ASC, m.id ASC LIMIT ?3"
                ))?;
                let rows = statement.query_map(
                    params![conversation_id, cursor_ic, query_limit],
                    |row| {
                        let message = map_message_view(row)?;
                        let sort_ic = row.get(10)?;
                        Ok((message, sort_ic))
                    },
                )?;
                let mut rows = rows.collect::<std::result::Result<Vec<_>, _>>()?;
                let has_more = rows.len() > page_limit;
                if has_more {
                    rows.truncate(page_limit);
                }
                let next_cursor = has_more
                    .then(|| rows.last())
                    .flatten()
                    .map(|(_, sort_ic)| {
                        encode_cursor(&MessageCursor {
                            view: mode.as_str().to_string(),
                            ic: Some(*sort_ic),
                            depth: None,
                            node_id: None,
                        })
                    })
                    .transpose()?;
                Ok(ReadPage {
                    items: rows.into_iter().map(|(message, _)| message).collect(),
                    next_cursor,
                })
            }
            MessageViewMode::CurrentThread => {
                let Some(meta) = self.conversation_meta(conversation_id)? else {
                    return Ok(ReadPage {
                        items: Vec::new(),
                        next_cursor: None,
                    });
                };
                let Some(current_node_id) = meta.current_node_id else {
                    let all_active_cursor = cursor
                        .as_ref()
                        .map(|value| {
                            encode_cursor(&MessageCursor {
                                view: MessageViewMode::AllActive.as_str().to_string(),
                                ic: value.ic,
                                depth: None,
                                node_id: None,
                            })
                        })
                        .transpose()?;
                    let mut page = self.message_page(
                        conversation_id,
                        MessageViewMode::AllActive,
                        page_limit,
                        all_active_cursor.as_deref(),
                    )?;
                    if let Some(next_cursor) = page.next_cursor.take() {
                        let mut decoded = decode_cursor::<MessageCursor>(Some(&next_cursor))?
                            .ok_or_else(|| RecallError::msg("invalid empty message cursor"))?;
                        decoded.view = MessageViewMode::CurrentThread.as_str().to_string();
                        page.next_cursor = Some(encode_cursor(&decoded)?);
                    }
                    return Ok(page);
                };
                let cursor_depth = cursor.as_ref().and_then(|value| value.depth);
                let cursor_node_id = cursor.as_ref().and_then(|value| value.node_id.as_deref());
                let mut statement = self.conn.prepare(&format!(
                    "WITH RECURSIVE path(id, parent_id, depth) AS (
                         SELECT id, parent_id, 0 FROM nodes WHERE id = ?1 AND is_active = 1
                         UNION ALL
                         SELECT n.id, n.parent_id, path.depth + 1
                         FROM nodes n JOIN path ON path.parent_id = n.id
                         WHERE n.is_active = 1
                     )
                     SELECT m.id,
                            CASE WHEN {TRUSTED_REFERENCE_ROLE_PREDICATE} THEN m.ic ELSE NULL END,
                            n.id, n.parent_id, m.conversation_id, COALESCE(m.role, 'unknown'), m.model_slug,
                            COALESCE((SELECT GROUP_CONCAT(text_content, char(10)) FROM (
                                SELECT cb.text_content FROM content_blocks cb
                                WHERE cb.message_id = m.id ORDER BY cb.ordinal
                            )), ''),
                            m.create_time, m.timestamp, p.depth, n.id
                     FROM path p JOIN nodes n ON n.id = p.id
                     JOIN messages m ON m.node_id = n.id AND m.is_active = 1
                     WHERE (?2 IS NULL OR p.depth < ?2
                       OR (p.depth = ?2 AND n.id > ?3))
                     ORDER BY p.depth DESC, n.id ASC LIMIT ?4"
                ))?;
                let rows = statement.query_map(
                    params![current_node_id, cursor_depth, cursor_node_id, query_limit],
                    |row| {
                        let message = map_message_view(row)?;
                        let depth: i64 = row.get(10)?;
                        let node_id: String = row.get(11)?;
                        Ok((message, depth, node_id))
                    },
                )?;
                let mut rows = rows.collect::<std::result::Result<Vec<_>, _>>()?;
                let has_more = rows.len() > page_limit;
                if has_more {
                    rows.truncate(page_limit);
                }
                let next_cursor = has_more
                    .then(|| rows.last())
                    .flatten()
                    .map(|(_, depth, node_id)| {
                        encode_cursor(&MessageCursor {
                            view: mode.as_str().to_string(),
                            ic: None,
                            depth: Some(*depth),
                            node_id: Some(node_id.clone()),
                        })
                    })
                    .transpose()?;
                Ok(ReadPage {
                    items: rows.into_iter().map(|(message, _, _)| message).collect(),
                    next_cursor,
                })
            }
        }
    }

    pub fn search(&self, term: &str, limit: usize) -> Result<Vec<SearchHit>> {
        let term = term.trim();
        if term.is_empty() {
            return Ok(Vec::new());
        }
        let fts_query = sanitize_fts_query(term);
        if fts_query.is_empty() {
            return Ok(Vec::new());
        }
        let limit = i64::try_from(limit.min(500)).unwrap_or(500);
        let fts_sql = format!(
            "SELECT m.conversation_id, COALESCE(c.title, 'Untitled'), m.id,
                    CASE WHEN {TRUSTED_REFERENCE_ROLE_PREDICATE} THEN m.ic ELSE NULL END,
                    COALESCE(m.role, 'unknown'), m.model_slug, n.id, n.parent_id,
                    cb.text_content, m.create_time, m.timestamp
             FROM content_blocks cb JOIN content_blocks_fts fts ON cb.rowid = fts.rowid
             JOIN messages m ON m.id = cb.message_id
             JOIN nodes n ON n.id = m.node_id
             JOIN conversations c ON c.id = m.conversation_id
             WHERE c.is_active = 1 AND m.is_active = 1
               AND fts.content_blocks_fts MATCH ?1
             ORDER BY COALESCE(m.create_time, 0) DESC, m.id, cb.ordinal ASC LIMIT ?2"
        );
        let mut statement = self.conn.prepare(&fts_sql)?;
        let rows = statement.query_map(params![fts_query, limit], |row| {
            Ok(SearchHit {
                conversation_id: row.get(0)?,
                conversation_title: row.get(1)?,
                message_id: row.get(2)?,
                ic: row.get(3)?,
                role: row.get(4)?,
                model_slug: row.get(5)?,
                node_id: row.get(6)?,
                parent_node_id: row.get(7)?,
                excerpt: row.get(8)?,
                created_at: row.get(9)?,
                timestamp: row.get(10)?,
            })
        })?;
        let hits = rows.collect::<std::result::Result<Vec<_>, _>>()?;
        if !hits.is_empty() {
            return Ok(hits);
        }

        let pattern = format!("%{term}%");
        let like_sql = format!(
            "SELECT m.conversation_id, COALESCE(c.title, 'Untitled'), m.id,
                    CASE WHEN {TRUSTED_REFERENCE_ROLE_PREDICATE} THEN m.ic ELSE NULL END,
                    COALESCE(m.role, 'unknown'), m.model_slug, n.id, n.parent_id,
                    cb.text_content, m.create_time, m.timestamp
             FROM content_blocks cb JOIN messages m ON m.id = cb.message_id
             JOIN nodes n ON n.id = m.node_id
             JOIN conversations c ON c.id = m.conversation_id
             WHERE c.is_active = 1 AND m.is_active = 1 AND cb.text_content LIKE ?1
             ORDER BY COALESCE(m.create_time, 0) DESC, m.id, cb.ordinal ASC LIMIT ?2"
        );
        let mut statement = self.conn.prepare(&like_sql)?;
        let rows = statement.query_map(params![pattern, limit], |row| {
            Ok(SearchHit {
                conversation_id: row.get(0)?,
                conversation_title: row.get(1)?,
                message_id: row.get(2)?,
                ic: row.get(3)?,
                role: row.get(4)?,
                model_slug: row.get(5)?,
                node_id: row.get(6)?,
                parent_node_id: row.get(7)?,
                excerpt: row.get(8)?,
                created_at: row.get(9)?,
                timestamp: row.get(10)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn search_page(
        &self,
        term: &str,
        limit: usize,
        cursor: Option<&str>,
    ) -> Result<ReadPage<SearchHit>> {
        let cursor = decode_cursor::<SearchCursor>(cursor)?;
        let term = term.trim();
        let fts_query = sanitize_fts_query(term);
        let page_limit = limit.min(500);
        if term.is_empty() || fts_query.is_empty() || page_limit == 0 {
            return Ok(ReadPage {
                items: Vec::new(),
                next_cursor: None,
            });
        }
        let query_limit = i64::try_from(page_limit + 1).unwrap_or(501);
        let cursor_time = cursor.as_ref().map(|value| value.created_at);
        let cursor_message_id = cursor.as_ref().map(|value| value.message_id.as_str());
        let fts_sql = format!(
            "SELECT m.conversation_id, COALESCE(c.title, 'Untitled'), m.id,
                    CASE WHEN {TRUSTED_REFERENCE_ROLE_PREDICATE} THEN m.ic ELSE NULL END,
                    COALESCE(m.role, 'unknown'), m.model_slug, n.id, n.parent_id,
                    COALESCE((SELECT cb2.text_content
                              FROM content_blocks cb2
                              JOIN content_blocks_fts fts2 ON cb2.rowid = fts2.rowid
                              WHERE cb2.message_id = m.id AND fts2.content_blocks_fts MATCH ?1
                              ORDER BY cb2.ordinal LIMIT 1), ''),
                    m.create_time, m.timestamp
             FROM messages m JOIN nodes n ON n.id = m.node_id
             JOIN conversations c ON c.id = m.conversation_id
             WHERE c.is_active = 1 AND m.is_active = 1
               AND EXISTS (
                 SELECT 1 FROM content_blocks cb
                 JOIN content_blocks_fts fts ON cb.rowid = fts.rowid
                 WHERE cb.message_id = m.id AND fts.content_blocks_fts MATCH ?1
               )
               AND (?3 IS NULL OR COALESCE(m.create_time, 0) < ?3
                 OR (COALESCE(m.create_time, 0) = ?3 AND m.id > ?4))
             ORDER BY COALESCE(m.create_time, 0) DESC, m.id ASC LIMIT ?2"
        );
        let mut statement = self.conn.prepare(&fts_sql)?;
        let mut rows = statement
            .query_map(
                params![fts_query, query_limit, cursor_time, cursor_message_id],
                map_search_hit,
            )?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if rows.is_empty() {
            let pattern = format!("%{term}%");
            let like_sql = format!(
                "SELECT m.conversation_id, COALESCE(c.title, 'Untitled'), m.id,
                        CASE WHEN {TRUSTED_REFERENCE_ROLE_PREDICATE} THEN m.ic ELSE NULL END,
                        COALESCE(m.role, 'unknown'), m.model_slug, n.id, n.parent_id,
                        COALESCE((SELECT cb2.text_content FROM content_blocks cb2
                                  WHERE cb2.message_id = m.id AND cb2.text_content LIKE ?1
                                  ORDER BY cb2.ordinal LIMIT 1), ''),
                        m.create_time, m.timestamp
                 FROM messages m JOIN nodes n ON n.id = m.node_id
                 JOIN conversations c ON c.id = m.conversation_id
                 WHERE c.is_active = 1 AND m.is_active = 1
                   AND EXISTS (
                     SELECT 1 FROM content_blocks cb
                     WHERE cb.message_id = m.id AND cb.text_content LIKE ?1
                   )
                   AND (?3 IS NULL OR COALESCE(m.create_time, 0) < ?3
                     OR (COALESCE(m.create_time, 0) = ?3 AND m.id > ?4))
                 ORDER BY COALESCE(m.create_time, 0) DESC, m.id ASC LIMIT ?2"
            );
            let mut statement = self.conn.prepare(&like_sql)?;
            rows = statement
                .query_map(
                    params![pattern, query_limit, cursor_time, cursor_message_id],
                    map_search_hit,
                )?
                .collect::<std::result::Result<Vec<_>, _>>()?;
        }
        let has_more = rows.len() > page_limit;
        if has_more {
            rows.truncate(page_limit);
        }
        let next_cursor = has_more
            .then(|| rows.last())
            .flatten()
            .map(|hit| {
                encode_cursor(&SearchCursor {
                    created_at: hit.created_at.unwrap_or(0.0),
                    message_id: hit.message_id.clone(),
                })
            })
            .transpose()?;
        Ok(ReadPage {
            items: rows,
            next_cursor,
        })
    }

    pub fn list_conversations_cursor(
        &self,
        term: &str,
        limit: usize,
        cursor: Option<&str>,
    ) -> Result<ReadPage<ConversationListItem>> {
        let cursor = decode_cursor::<ConversationCursor>(cursor)?;
        let page_limit = limit.min(500);
        if page_limit == 0 {
            return Ok(ReadPage {
                items: Vec::new(),
                next_cursor: None,
            });
        }
        let term = term.trim();
        let pattern = format!("%{term}%");
        let fts_query = sanitize_fts_query(term);
        let query_limit = i64::try_from(page_limit + 1).unwrap_or(501);
        let cursor_time = cursor.as_ref().map(|value| value.sort_time);
        let cursor_id = cursor.as_ref().map(|value| value.id.as_str());
        let mut statement = self.conn.prepare(
            "SELECT c.id, COALESCE(c.title, 'Untitled'), c.update_time,
                    COUNT(m.id),
                    COALESCE((SELECT cb.text_content
                      FROM messages lm JOIN content_blocks cb ON cb.message_id = lm.id
                      WHERE lm.conversation_id = c.id AND lm.is_active = 1 AND cb.text_content IS NOT NULL
                      ORDER BY COALESCE(lm.create_time, 0) DESC, cb.ordinal LIMIT 1), ''),
                    c.default_model_slug,
                    EXISTS(
                      SELECT 1 FROM nodes branch_nodes
                      WHERE branch_nodes.conversation_id = c.id AND branch_nodes.is_active = 1
                        AND branch_nodes.parent_id IS NOT NULL
                      GROUP BY branch_nodes.parent_id HAVING COUNT(*) > 1
                    ),
                    COALESCE(c.update_time, c.create_time, 0)
             FROM conversations c LEFT JOIN messages m ON m.conversation_id = c.id AND m.is_active = 1
             WHERE c.is_active = 1 AND (?1 = '' OR c.title LIKE ?2
               OR (?4 <> '' AND EXISTS (SELECT 1 FROM conversations_fts fts WHERE fts.rowid = c.rowid AND fts.conversations_fts MATCH ?4))
               OR EXISTS (
                 SELECT 1 FROM messages sm JOIN content_blocks sb ON sb.message_id = sm.id
                 WHERE sm.conversation_id = c.id AND sm.is_active = 1 AND sb.text_content LIKE ?2
               ))
               AND (?6 IS NULL OR COALESCE(c.update_time, c.create_time, 0) < ?6
                 OR (COALESCE(c.update_time, c.create_time, 0) = ?6 AND c.id > ?7))
             GROUP BY c.id
             ORDER BY COALESCE(c.update_time, c.create_time, 0) DESC, c.id ASC
             LIMIT ?3",
        )?;
        let rows = statement.query_map(
            params![
                term,
                pattern,
                query_limit,
                fts_query,
                cursor_time,
                cursor_time,
                cursor_id,
            ],
            |row| {
                Ok(ConversationListItem {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    updated_at: row.get(2)?,
                    message_count: row.get(3)?,
                    excerpt: row.get(4)?,
                    default_model_slug: row.get(5)?,
                    has_branches: row.get::<_, i64>(6)? != 0,
                    sort_time: row.get(7)?,
                })
            },
        )?;
        let mut items = rows.collect::<std::result::Result<Vec<_>, _>>()?;
        let has_more = items.len() > page_limit;
        if has_more {
            items.truncate(page_limit);
        }
        let next_cursor = has_more
            .then(|| items.last())
            .flatten()
            .map(|item| {
                encode_cursor(&ConversationCursor {
                    sort_time: item.sort_time,
                    id: item.id.clone(),
                })
            })
            .transpose()?;
        Ok(ReadPage { items, next_cursor })
    }

    pub fn resolve_ic_jump(&self, ic: i64) -> Result<Option<IcJumpTarget>> {
        if ic <= 0 {
            return Ok(None);
        }
        self.conn
            .query_row(
                &format!(
                    "SELECT m.id, m.conversation_id
                     FROM messages m
                     JOIN conversations c ON c.id = m.conversation_id
                     WHERE m.ic = ?1 AND m.is_active = 1 AND c.is_active = 1
                       AND {TRUSTED_REFERENCE_ROLE_PREDICATE}
                     LIMIT 1"
                ),
                [ic],
                |row| {
                    Ok(IcJumpTarget {
                        message_id: row.get(0)?,
                        conversation_id: row.get(1)?,
                    })
                },
            )
            .optional()
            .map_err(Into::into)
    }

    pub fn resolve_ic_message(&self, ic: i64) -> Result<Option<MessageView>> {
        if ic <= 0 {
            return Ok(None);
        }
        let mut statement = self.conn.prepare(&format!(
            "SELECT m.id, m.ic, n.id, n.parent_id, m.conversation_id, COALESCE(m.role, 'unknown'), m.model_slug,
                    COALESCE((SELECT GROUP_CONCAT(text_content, char(10)) FROM (
                        SELECT cb.text_content FROM content_blocks cb
                        WHERE cb.message_id = m.id ORDER BY cb.ordinal
                    )), ''), m.create_time, m.timestamp
             FROM messages m JOIN nodes n ON n.id = m.node_id
             JOIN conversations c ON c.id = m.conversation_id
             WHERE m.ic = ?1 AND m.is_active = 1 AND n.is_active = 1 AND c.is_active = 1
               AND {TRUSTED_REFERENCE_ROLE_PREDICATE} LIMIT 1"
        ))?;
        statement
            .query_row([ic], map_message_view)
            .optional()
            .map_err(Into::into)
    }

    pub fn message_by_id(&self, message_id: &str) -> Result<Option<MessageView>> {
        let mut statement = self.conn.prepare(&format!(
            "SELECT m.id,
                    CASE WHEN {TRUSTED_REFERENCE_ROLE_PREDICATE} THEN m.ic ELSE NULL END,
                    n.id, n.parent_id, m.conversation_id, COALESCE(m.role, 'unknown'), m.model_slug,
                    COALESCE((SELECT GROUP_CONCAT(text_content, char(10)) FROM (
                        SELECT cb.text_content FROM content_blocks cb
                        WHERE cb.message_id = m.id ORDER BY cb.ordinal
                    )), ''), m.create_time, m.timestamp
             FROM messages m JOIN nodes n ON n.id = m.node_id
             JOIN conversations c ON c.id = m.conversation_id
             WHERE m.id = ?1 AND m.is_active = 1 AND n.is_active = 1 AND c.is_active = 1
             LIMIT 1"
        ))?;
        statement
            .query_row([message_id], map_message_view)
            .optional()
            .map_err(Into::into)
    }

    pub fn resolve_message_id(&self, message_id: &str) -> Result<Option<ReferencedMessage>> {
        get_active_message_by_id(&self.conn, message_id)
    }

    pub fn resolve_ic_reference(&self, ic: i64) -> Result<Option<ReferencedMessage>> {
        if ic <= 0 {
            return Ok(None);
        }
        get_active_message_by_ic(&self.conn, ic)
    }

    pub fn resolve_reference(
        &self,
        reference: &MessageReference,
    ) -> Result<Option<ReferencedMessage>> {
        resolve_message_reference(&self.conn, reference)
    }

    pub fn resolve_message_id_jump(&self, message_id: &str) -> Result<Option<IcJumpTarget>> {
        Ok(self
            .resolve_message_id(message_id)?
            .map(|message| IcJumpTarget {
                message_id: message.id,
                conversation_id: message.conversation_id,
            }))
    }

    pub fn resolve_reference_jump(
        &self,
        reference: &MessageReference,
    ) -> Result<Option<IcJumpTarget>> {
        Ok(self
            .resolve_reference(reference)?
            .map(|message| IcJumpTarget {
                message_id: message.id,
                conversation_id: message.conversation_id,
            }))
    }

    pub fn ic_context(&self, ic: i64) -> Result<Option<IcContext>> {
        get_ic_context(&self.conn, ic, 2, 2, ContextScope::Conversation)
    }

    pub fn ic_context_window(
        &self,
        ic: i64,
        before: usize,
        after: usize,
        scope: ContextScope,
    ) -> Result<Option<IcContext>> {
        get_ic_context(&self.conn, ic, before, after, scope)
    }

    pub fn message_window(
        &self,
        message_id: &str,
        before: usize,
        after: usize,
        mode: MessageViewMode,
    ) -> Result<Option<MessageWindow>> {
        let Some(target) = self.message_by_id(message_id)? else {
            return Ok(None);
        };
        match mode {
            MessageViewMode::AllActive => self.message_window_all_active(&target, before, after),
            MessageViewMode::CurrentThread => {
                let Some(meta) = self.conversation_meta(&target.conversation_id)? else {
                    return Ok(None);
                };
                let Some(current_node_id) = meta.current_node_id else {
                    return self.message_window_all_active(&target, before, after);
                };
                self.message_window_current_thread(&target, &current_node_id, before, after)
            }
        }
    }

    fn message_window_all_active(
        &self,
        target: &MessageView,
        before: usize,
        after: usize,
    ) -> Result<Option<MessageWindow>> {
        let before = i64::try_from(before).unwrap_or(50);
        let after = i64::try_from(after).unwrap_or(50);
        let mut statement = self.conn.prepare(&format!(
            "WITH ordered AS (
                 SELECT m.id,
                        ROW_NUMBER() OVER (ORDER BY m.ic ASC, m.id ASC) AS position
                 FROM messages m JOIN nodes n ON n.id = m.node_id
                 WHERE m.conversation_id = ?1 AND m.is_active = 1 AND n.is_active = 1
             ), target AS (
                 SELECT position FROM ordered WHERE id = ?2
             )
             SELECT m.id,
                    CASE WHEN {TRUSTED_REFERENCE_ROLE_PREDICATE} THEN m.ic ELSE NULL END,
                    n.id, n.parent_id, m.conversation_id, COALESCE(m.role, 'unknown'), m.model_slug,
                    COALESCE((SELECT GROUP_CONCAT(text_content, char(10)) FROM (
                        SELECT cb.text_content FROM content_blocks cb
                        WHERE cb.message_id = m.id ORDER BY cb.ordinal
                    )), ''), m.create_time, m.timestamp
             FROM ordered o JOIN target t
             JOIN messages m ON m.id = o.id
             JOIN nodes n ON n.id = m.node_id
             WHERE o.position BETWEEN t.position - ?3 AND t.position + ?4
             ORDER BY o.position"
        ))?;
        let rows = statement.query_map(
            params![target.conversation_id, target.id, before, after],
            map_message_view,
        )?;
        split_window(target, rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    fn message_window_current_thread(
        &self,
        target: &MessageView,
        current_node_id: &str,
        before: usize,
        after: usize,
    ) -> Result<Option<MessageWindow>> {
        let before = i64::try_from(before).unwrap_or(50);
        let after = i64::try_from(after).unwrap_or(50);
        let mut statement = self.conn.prepare(&format!(
            "WITH RECURSIVE path(id, parent_id, depth) AS (
                 SELECT id, parent_id, 0 FROM nodes WHERE id = ?1 AND is_active = 1
                 UNION ALL
                 SELECT n.id, n.parent_id, path.depth + 1
                 FROM nodes n JOIN path ON path.parent_id = n.id
                 WHERE n.is_active = 1
             ), ordered AS (
                 SELECT n.id AS node_id, m.id AS message_id,
                        ROW_NUMBER() OVER (ORDER BY p.depth DESC, n.id ASC) AS position
                 FROM path p JOIN nodes n ON n.id = p.id
                 JOIN messages m ON m.node_id = n.id AND m.is_active = 1
             ), target AS (
                 SELECT position FROM ordered WHERE message_id = ?2
             )
             SELECT m.id,
                    CASE WHEN {TRUSTED_REFERENCE_ROLE_PREDICATE} THEN m.ic ELSE NULL END,
                    n.id, n.parent_id, m.conversation_id, COALESCE(m.role, 'unknown'), m.model_slug,
                    COALESCE((SELECT GROUP_CONCAT(text_content, char(10)) FROM (
                        SELECT cb.text_content FROM content_blocks cb
                        WHERE cb.message_id = m.id ORDER BY cb.ordinal
                    )), ''), m.create_time, m.timestamp
             FROM ordered o JOIN target t
             JOIN messages m ON m.id = o.message_id
             JOIN nodes n ON n.id = m.node_id
             WHERE o.position BETWEEN t.position - ?3 AND t.position + ?4
             ORDER BY o.position"
        ))?;
        let rows = statement.query_map(
            params![current_node_id, target.id, before, after],
            map_message_view,
        )?;
        split_window(target, rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn latest_snapshot_id(&self) -> Result<Option<String>> {
        self.conn
            .query_row(
                "SELECT id FROM import_runs
                 WHERE status IN ('completed', 'partial') AND completed_at IS NOT NULL
                 ORDER BY completed_at DESC, id DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map(|id| id.map(|value: String| format!("import-run:{value}")))
            .map_err(Into::into)
    }

    pub fn branches_for_parent(&self, parent_node_id: &str) -> Result<Vec<BranchChoice>> {
        let mut statement = self.conn.prepare(&format!(
            "SELECT n.id, m.id,
                    CASE WHEN {TRUSTED_REFERENCE_ROLE_PREDICATE} THEN m.ic ELSE NULL END,
                    m.role,
                    COALESCE((SELECT cb.text_content FROM content_blocks cb
                              WHERE cb.message_id = m.id ORDER BY cb.ordinal LIMIT 1), '')
             FROM nodes n LEFT JOIN messages m ON m.node_id = n.id AND m.is_active = 1
             WHERE n.parent_id = ?1 AND n.is_active = 1
             ORDER BY n.rowid"
        ))?;
        let rows = statement.query_map([parent_node_id], |row| {
            Ok(BranchChoice {
                node_id: row.get(0)?,
                message_id: row.get(1)?,
                ic: row.get(2)?,
                role: row.get(3)?,
                preview: row.get(4)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn branches_on_path(&self, node_id: &str) -> Result<Vec<BranchChoice>> {
        let parent: Option<String> = self
            .conn
            .query_row(
                "WITH RECURSIVE path(id, parent_id, depth) AS (
                     SELECT id, parent_id, 0 FROM nodes WHERE id = ?1 AND is_active = 1
                     UNION ALL
                     SELECT n.id, n.parent_id, path.depth + 1
                     FROM nodes n JOIN path ON path.parent_id = n.id
                     WHERE n.is_active = 1
                 )
                 SELECT p.id FROM path p
                 WHERE (SELECT COUNT(*) FROM nodes child WHERE child.parent_id = p.id AND child.is_active = 1) > 1
                 ORDER BY p.depth ASC LIMIT 1",
                [node_id],
                |row| row.get(0),
            )
            .optional()?;
        parent.map_or_else(
            || Ok(Vec::new()),
            |parent| self.branches_for_parent(&parent),
        )
    }

    pub fn message_assets(&self, message_id: &str) -> Result<Vec<AssetView>> {
        let mut statement = self.conn.prepare(
            "SELECT a.id, COALESCE(a.display_name, a.source_filename, a.source_key),
                    a.mime_type, a.exists_locally, a.relative_path
             FROM assets a JOIN message_assets ma ON ma.asset_id = a.id
             WHERE ma.message_id = ?1 AND a.is_active = 1
             ORDER BY ma.ordinal",
        )?;
        let rows = statement.query_map([message_id], |row| {
            Ok(AssetView {
                id: row.get(0)?,
                name: row.get(1)?,
                mime_type: row.get(2)?,
                exists_locally: row.get::<_, i64>(3)? != 0,
                relative_path: row.get(4)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn conversation_assets(&self, conversation_id: &str) -> Result<Vec<AssetView>> {
        let mut statement = self.conn.prepare(
            "SELECT a.id, COALESCE(a.display_name, a.source_filename, a.source_key),
                    a.mime_type, a.exists_locally, a.relative_path
             FROM assets a JOIN message_assets ma ON ma.asset_id = a.id
             JOIN messages m ON m.id = ma.message_id
             WHERE m.conversation_id = ?1 AND a.is_active = 1
             GROUP BY a.id ORDER BY ma.ordinal",
        )?;
        let rows = statement.query_map([conversation_id], |row| {
            Ok(AssetView {
                id: row.get(0)?,
                name: row.get(1)?,
                mime_type: row.get(2)?,
                exists_locally: row.get::<_, i64>(3)? != 0,
                relative_path: row.get(4)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }

    pub fn conversation_graph(&self, conversation_id: &str) -> Result<Option<Vec<GraphNodeView>>> {
        if self.conversation_meta(conversation_id)?.is_none() {
            return Ok(None);
        }
        let mut statement = self.conn.prepare(&format!(
            "SELECT n.id, n.parent_id, m.id,
                    CASE WHEN {TRUSTED_REFERENCE_ROLE_PREDICATE} THEN m.ic ELSE NULL END,
                    m.role,
                    COALESCE((SELECT cb.text_content FROM content_blocks cb
                              WHERE cb.message_id = m.id ORDER BY cb.ordinal LIMIT 1), '')
             FROM nodes n LEFT JOIN messages m ON m.node_id = n.id AND m.is_active = 1
             WHERE n.conversation_id = ?1 AND n.is_active = 1
             ORDER BY n.rowid LIMIT 800"
        ))?;
        let rows = statement.query_map([conversation_id], |row| {
            Ok(GraphNodeView {
                id: row.get(0)?,
                parent_id: row.get(1)?,
                message_id: row.get(2)?,
                ic: row.get(3)?,
                role: row.get(4)?,
                preview: row.get(5)?,
            })
        })?;
        Ok(Some(rows.collect::<std::result::Result<Vec<_>, _>>()?))
    }

    pub fn list_assets(&self, term: &str, limit: usize) -> Result<Vec<AssetListItem>> {
        let term = term.trim();
        let pattern = format!("%{term}%");
        let limit = i64::try_from(limit.min(500)).unwrap_or(500);
        let mut statement = self.conn.prepare(
            "SELECT a.id, COALESCE(a.display_name, a.source_filename, a.source_key), a.mime_type,
                    a.exists_locally, m.conversation_id, COALESCE(c.title, 'Untitled')
             FROM assets a JOIN message_assets ma ON ma.asset_id = a.id
             JOIN messages m ON m.id = ma.message_id JOIN conversations c ON c.id = m.conversation_id
             WHERE a.is_active = 1 AND m.is_active = 1 AND c.is_active = 1
               AND (?1 = '' OR a.display_name LIKE ?2 OR a.source_filename LIKE ?2 OR c.title LIKE ?2)
             GROUP BY a.id ORDER BY a.rowid DESC LIMIT ?3",
        )?;
        let rows = statement.query_map(params![term, pattern, limit], |row| {
            Ok(AssetListItem {
                id: row.get(0)?,
                name: row.get(1)?,
                mime_type: row.get(2)?,
                exists_locally: row.get::<_, i64>(3)? != 0,
                conversation_id: row.get(4)?,
                conversation_title: row.get(5)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
    }
}

impl MessageViewMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::CurrentThread => "current-thread",
            Self::AllActive => "all-active",
        }
    }
}

fn decode_cursor<T: DeserializeOwned>(cursor: Option<&str>) -> Result<Option<T>> {
    cursor
        .map(|value| {
            serde_json::from_str(value)
                .map_err(|error| RecallError::msg(format!("invalid cursor: {error}")))
        })
        .transpose()
}

fn encode_cursor<T: Serialize>(cursor: &T) -> Result<String> {
    serde_json::to_string(cursor).map_err(Into::into)
}

fn split_window(target: &MessageView, messages: Vec<MessageView>) -> Result<Option<MessageWindow>> {
    let Some(center_index) = messages.iter().position(|message| message.id == target.id) else {
        return Ok(None);
    };
    Ok(Some(MessageWindow {
        center: messages[center_index].clone(),
        before: messages[..center_index].to_vec(),
        after: messages[center_index + 1..].to_vec(),
    }))
}

fn map_message_view(row: &rusqlite::Row<'_>) -> rusqlite::Result<MessageView> {
    Ok(MessageView {
        id: row.get(0)?,
        ic: row.get(1)?,
        node_id: row.get(2)?,
        parent_node_id: row.get(3)?,
        conversation_id: row.get(4)?,
        role: row.get(5)?,
        model_slug: row.get(6)?,
        content: row.get(7)?,
        created_at: row.get(8)?,
        timestamp: row.get(9)?,
    })
}

fn map_search_hit(row: &rusqlite::Row<'_>) -> rusqlite::Result<SearchHit> {
    Ok(SearchHit {
        conversation_id: row.get(0)?,
        conversation_title: row.get(1)?,
        message_id: row.get(2)?,
        ic: row.get(3)?,
        role: row.get(4)?,
        model_slug: row.get(5)?,
        node_id: row.get(6)?,
        parent_node_id: row.get(7)?,
        excerpt: row.get(8)?,
        created_at: row.get(9)?,
        timestamp: row.get(10)?,
    })
}

pub fn sanitize_fts_query(q: &str) -> String {
    q.split_whitespace()
        .filter_map(|word| {
            let cleaned: String = word.chars().filter(|c| c.is_alphanumeric()).collect();
            (!cleaned.is_empty()).then(|| format!("\"{cleaned}\"*"))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::sanitize_fts_query;

    #[test]
    fn sanitizes_fts_input() {
        assert_eq!(sanitize_fts_query("hello rust"), "\"hello\"* \"rust\"*");
        assert_eq!(sanitize_fts_query("***"), "");
    }
}
