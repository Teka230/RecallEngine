use std::path::Path;

use rusqlite::{params, Connection, OpenFlags, OptionalExtension};

use crate::domain::reference::{
    get_active_message_by_ic, get_active_message_by_id, get_ic_context, is_reference_role,
    resolve_message_reference, ContextScope, IcContext, MessageReference, ReferencedMessage,
};
use crate::error::{RecallError, Result};

const REFERENCE_ROLE_SQL: &str = "LOWER(TRIM(COALESCE(m.role, ''))) IN ('user', 'assistant')";

#[derive(Debug, Clone, PartialEq)]
pub struct ConversationListItem {
    pub id: String,
    pub title: String,
    pub updated_at: Option<f64>,
    pub message_count: i64,
    pub excerpt: String,
    pub has_branches: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConversationMeta {
    pub id: String,
    pub title: String,
    pub current_node_id: Option<String>,
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

pub struct ReadRepository {
    conn: Connection,
}

impl ReadRepository {
    pub fn open_read_only(path: &Path) -> Result<Self> {
        if !path.is_file() {
            return Err(RecallError::msg("Database not found"));
        }
        let conn = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        let repository = Self { conn };
        repository.validate_schema()?;
        Ok(repository)
    }

    fn validate_schema(&self) -> Result<()> {
        for table in ["conversations", "nodes", "messages", "content_blocks"] {
            let exists: i64 = self.conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1)",
                [table],
                |row| row.get(0),
            )?;
            if exists == 0 {
                return Err(RecallError::msg("Unsupported RecallEngine database schema"));
            }
        }
        let has_ic = self
            .conn
            .prepare("PRAGMA table_info(messages)")?
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<std::result::Result<Vec<_>, _>>()?
            .iter()
            .any(|column| column == "ic");
        if !has_ic {
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
                    )
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
                "SELECT id, COALESCE(title, 'Untitled'), current_node_id
                 FROM conversations WHERE id = ?1 AND is_active = 1",
                [id],
                |row| {
                    Ok(ConversationMeta {
                        id: row.get(0)?,
                        title: row.get(1)?,
                        current_node_id: row.get(2)?,
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
                    CASE WHEN {REFERENCE_ROLE_SQL} THEN m.ic ELSE NULL END,
                    n.id, n.parent_id, m.conversation_id, COALESCE(m.role, 'unknown'),
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
                    CASE WHEN {REFERENCE_ROLE_SQL} THEN m.ic ELSE NULL END,
                    n.id, n.parent_id, m.conversation_id, COALESCE(m.role, 'unknown'),
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
                    CASE WHEN {REFERENCE_ROLE_SQL} THEN m.ic ELSE NULL END,
                    n.id, n.parent_id, m.conversation_id, COALESCE(m.role, 'unknown'),
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
                    CASE WHEN {REFERENCE_ROLE_SQL} THEN m.ic ELSE NULL END,
                    COALESCE(m.role, 'unknown'), cb.text_content, m.create_time, m.timestamp
             FROM content_blocks cb JOIN content_blocks_fts fts ON cb.rowid = fts.rowid
             JOIN messages m ON m.id = cb.message_id
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
                excerpt: row.get(5)?,
                created_at: row.get(6)?,
                timestamp: row.get(7)?,
            })
        })?;
        let hits = rows.collect::<std::result::Result<Vec<_>, _>>()?;
        if !hits.is_empty() {
            return Ok(hits);
        }

        let pattern = format!("%{term}%");
        let like_sql = format!(
            "SELECT m.conversation_id, COALESCE(c.title, 'Untitled'), m.id,
                    CASE WHEN {REFERENCE_ROLE_SQL} THEN m.ic ELSE NULL END,
                    COALESCE(m.role, 'unknown'), cb.text_content, m.create_time, m.timestamp
             FROM content_blocks cb JOIN messages m ON m.id = cb.message_id
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
                excerpt: row.get(5)?,
                created_at: row.get(6)?,
                timestamp: row.get(7)?,
            })
        })?;
        Ok(rows.collect::<std::result::Result<Vec<_>, _>>()?)
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
                       AND {REFERENCE_ROLE_SQL}
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
            "SELECT m.id, m.ic, n.id, n.parent_id, m.conversation_id, COALESCE(m.role, 'unknown'),
                    COALESCE((SELECT GROUP_CONCAT(text_content, char(10)) FROM (
                        SELECT cb.text_content FROM content_blocks cb
                        WHERE cb.message_id = m.id ORDER BY cb.ordinal
                    )), ''), m.create_time, m.timestamp
             FROM messages m JOIN nodes n ON n.id = m.node_id
             JOIN conversations c ON c.id = m.conversation_id
             WHERE m.ic = ?1 AND m.is_active = 1 AND n.is_active = 1 AND c.is_active = 1
               AND {REFERENCE_ROLE_SQL} LIMIT 1"
        ))?;
        statement
            .query_row([ic], map_message_view)
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

    pub fn branches_for_parent(&self, parent_node_id: &str) -> Result<Vec<BranchChoice>> {
        let mut statement = self.conn.prepare(&format!(
            "SELECT n.id, m.id,
                    CASE WHEN {REFERENCE_ROLE_SQL} THEN m.ic ELSE NULL END,
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
                    CASE WHEN {REFERENCE_ROLE_SQL} THEN m.ic ELSE NULL END,
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

fn map_message_view(row: &rusqlite::Row<'_>) -> rusqlite::Result<MessageView> {
    Ok(MessageView {
        id: row.get(0)?,
        ic: row.get(1)?,
        node_id: row.get(2)?,
        parent_node_id: row.get(3)?,
        conversation_id: row.get(4)?,
        role: row.get(5)?,
        content: row.get(6)?,
        created_at: row.get(7)?,
        timestamp: row.get(8)?,
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
