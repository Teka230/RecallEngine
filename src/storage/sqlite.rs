use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension, Transaction};
use time::OffsetDateTime;
use uuid::Uuid;

use crate::domain::canonical::{
    ContentBlockRecord, ContentReferenceRecord, ConversationRecord, MessageCandidate, NodeRecord,
};
use crate::error::Result;
use crate::storage::schema;
use crate::storage::sql_idents::{FragmentTable, SidecarTable};

#[derive(Debug, Clone)]
pub struct ImportIssue {
    pub severity: &'static str,
    pub code: &'static str,
    pub entity_type: Option<String>,
    pub entity_id: Option<String>,
    pub source_relative_path: Option<String>,
    pub message: String,
}

pub struct Database {
    conn: Connection,
}

#[derive(Debug, Clone)]
pub struct AssetUpsert<'a> {
    pub run_id: &'a str,
    pub id: &'a str,
    pub source_key: &'a str,
    pub display_name: Option<&'a str>,
    pub relative_path: Option<&'a str>,
    pub mime_type: Option<&'a str>,
    pub size_bytes: Option<i64>,
    pub exists_locally: bool,
    pub raw_json: &'a str,
}

#[derive(Debug, Clone)]
pub struct FeedbackUpsert<'a> {
    pub run_id: &'a str,
    pub id: &'a str,
    pub message_id: Option<&'a str>,
    pub rating: Option<&'a str>,
    pub tags: Option<&'a str>,
    pub text: Option<&'a str>,
    pub created_at: Option<&'a str>,
    pub raw_json: &'a str,
}

#[derive(Debug, Clone)]
pub struct SharedUpsert<'a> {
    pub run_id: &'a str,
    pub id: &'a str,
    pub conversation_id: Option<&'a str>,
    pub share_id: Option<&'a str>,
    pub url: Option<&'a str>,
    pub created_at: Option<&'a str>,
    pub is_anonymous: i32,
    pub raw_json: &'a str,
}

#[derive(Debug, Clone)]
pub struct LibraryFileUpsert<'a> {
    pub run_id: &'a str,
    pub id: &'a str,
    pub file_id: Option<&'a str>,
    pub file_name: Option<&'a str>,
    pub mime_type: Option<&'a str>,
    pub file_size_bytes: Option<i64>,
    pub sha256_digest: Option<&'a str>,
    pub raw_json: &'a str,
}

impl Database {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        // Journal mode: DELETE/rollback (PR-H5 Outcome A). Do not enable WAL here —
        // see tests/sqlite_concurrency.rs and the README concurrency note.
        conn.execute("PRAGMA foreign_keys = ON", [])?;
        schema::apply_migrations(&conn)?;
        Ok(Self { conn })
    }

    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    pub fn begin(&mut self) -> Result<Transaction<'_>> {
        Ok(self.conn.transaction()?)
    }

    pub fn create_import_run(&self, source_root: &str, strict: bool) -> Result<String> {
        let id = Uuid::new_v4().to_string();
        let started = OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default();
        self.conn.execute(
            "INSERT INTO import_runs (id, source_root, started_at, status, strict_mode)
             VALUES (?1, ?2, ?3, 'running', ?4)",
            params![id, source_root, started, i32::from(strict)],
        )?;
        Ok(id)
    }

    pub fn finish_import_run(
        &self,
        run_id: &str,
        status: &str,
        stats_json: Option<&str>,
        error_summary: Option<&str>,
    ) -> Result<()> {
        let completed = OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default();
        self.conn.execute(
            "UPDATE import_runs SET completed_at = ?1, status = ?2, stats_json = ?3, error_summary = ?4
             WHERE id = ?5",
            params![completed, status, stats_json, error_summary, run_id],
        )?;
        Ok(())
    }

    pub fn record_source_file(
        &self,
        run_id: &str,
        relative_path: &str,
        kind: &str,
        size_bytes: i64,
        sha256: &str,
        status: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO source_files (import_run_id, relative_path, kind, size_bytes, sha256, status)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT(import_run_id, relative_path) DO UPDATE SET
               size_bytes = excluded.size_bytes,
               sha256 = excluded.sha256,
               status = excluded.status",
            params![run_id, relative_path, kind, size_bytes, sha256, status],
        )?;
        Ok(())
    }

    pub fn last_completed_hash(
        &self,
        source_root: &str,
        relative_path: &str,
    ) -> Result<Option<String>> {
        let hash: Option<String> = self
            .conn
            .query_row(
                "SELECT sf.sha256 FROM source_files sf
                 JOIN import_runs ir ON ir.id = sf.import_run_id
                 WHERE ir.source_root = ?1 AND ir.status = 'completed'
                   AND sf.relative_path = ?2 AND sf.status IN ('imported', 'skipped')
                 ORDER BY ir.completed_at DESC LIMIT 1",
                params![source_root, relative_path],
                |r| r.get(0),
            )
            .optional()?;
        Ok(hash)
    }

    pub fn insert_issue(&self, run_id: &str, issue: &ImportIssue) -> Result<()> {
        let now = OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default();
        self.conn.execute(
            "INSERT INTO import_issues
             (import_run_id, severity, code, entity_type, entity_id, source_relative_path, message, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                run_id,
                issue.severity,
                issue.code,
                issue.entity_type,
                issue.entity_id,
                issue.source_relative_path,
                issue.message,
                now,
            ],
        )?;
        Ok(())
    }

    pub fn upsert_conversation(&self, run_id: &str, c: &ConversationRecord) -> Result<()> {
        self.conn.execute(
            "INSERT INTO conversations
             (id, title, create_time, update_time, current_node_id, default_model_slug,
              is_archived, is_starred, source_relative_path, last_seen_import_run_id, is_active, raw_json)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,1,?11)
             ON CONFLICT(id) DO UPDATE SET
               title = excluded.title,
               create_time = excluded.create_time,
               update_time = excluded.update_time,
               current_node_id = excluded.current_node_id,
               default_model_slug = excluded.default_model_slug,
               is_archived = excluded.is_archived,
               is_starred = excluded.is_starred,
               source_relative_path = excluded.source_relative_path,
               last_seen_import_run_id = excluded.last_seen_import_run_id,
               is_active = 1,
               raw_json = excluded.raw_json",
            params![
                c.id,
                c.title,
                c.create_time,
                c.update_time,
                c.current_node_id,
                c.default_model_slug,
                c.is_archived,
                c.is_starred,
                c.source_relative_path,
                run_id,
                c.raw_json,
            ],
        )?;
        Ok(())
    }

    pub fn upsert_node(&self, run_id: &str, n: &NodeRecord) -> Result<()> {
        self.conn.execute(
            "INSERT INTO nodes
             (id, conversation_id, parent_id, has_message, source_relative_path, last_seen_import_run_id, is_active, raw_json)
             VALUES (?1,?2,?3,?4,?5,?6,1,?7)
             ON CONFLICT(id) DO UPDATE SET
               conversation_id = excluded.conversation_id,
               parent_id = excluded.parent_id,
               has_message = excluded.has_message,
               source_relative_path = excluded.source_relative_path,
               last_seen_import_run_id = excluded.last_seen_import_run_id,
               is_active = 1,
               raw_json = excluded.raw_json",
            params![
                n.id,
                n.conversation_id,
                n.parent_id,
                i32::from(n.has_message),
                n.source_relative_path,
                run_id,
                n.raw_json,
            ],
        )?;
        Ok(())
    }

    pub fn upsert_message(&self, run_id: &str, m: &MessageCandidate, ic: i64) -> Result<()> {
        self.conn.execute(
            "INSERT INTO messages
             (id, ic, node_id, conversation_id, role, author_name, create_time, create_time_raw, timestamp,
              source_shard_index, source_node_order, model_slug, content_type, source_relative_path,
              last_seen_import_run_id, is_active, raw_json)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,1,?16)
             ON CONFLICT(id) DO UPDATE SET
               node_id = excluded.node_id,
               conversation_id = excluded.conversation_id,
               role = excluded.role,
               author_name = excluded.author_name,
               create_time = excluded.create_time,
               create_time_raw = excluded.create_time_raw,
               timestamp = excluded.timestamp,
               source_shard_index = excluded.source_shard_index,
               source_node_order = excluded.source_node_order,
               model_slug = excluded.model_slug,
               content_type = excluded.content_type,
               source_relative_path = excluded.source_relative_path,
               last_seen_import_run_id = excluded.last_seen_import_run_id,
               is_active = 1,
               raw_json = excluded.raw_json",
            params![
                m.id,
                ic,
                m.node_id,
                m.conversation_id,
                m.role,
                m.author_name,
                m.create_time,
                m.create_time_raw,
                m.timestamp,
                m.source_shard_index,
                m.source_node_order,
                m.model_slug,
                m.content_type,
                m.source_relative_path,
                run_id,
                m.raw_json,
            ],
        )?;
        Ok(())
    }

    pub fn upsert_content_block(&self, b: &ContentBlockRecord) -> Result<()> {
        self.conn.execute(
            "INSERT INTO content_blocks (id, message_id, ordinal, kind, text_content, json_content)
             VALUES (?1,?2,?3,?4,?5,?6)
             ON CONFLICT(message_id, ordinal) DO UPDATE SET
               kind = excluded.kind,
               text_content = excluded.text_content,
               json_content = excluded.json_content",
            params![
                b.id,
                b.message_id,
                b.ordinal,
                b.kind,
                b.text_content,
                b.json_content
            ],
        )?;
        Ok(())
    }

    pub fn upsert_content_reference(&self, r: &ContentReferenceRecord) -> Result<()> {
        self.conn.execute(
            "INSERT INTO content_references (id, message_id, ordinal, ref_source, raw_json)
             VALUES (?1,?2,?3,?4,?5)
             ON CONFLICT(message_id, ordinal, ref_source) DO UPDATE SET raw_json = excluded.raw_json",
            params![r.id, r.message_id, r.ordinal, r.ref_source, r.raw_json],
        )?;
        Ok(())
    }

    pub fn reconcile_fragment(&self, run_id: &str, relative_path: &str) -> Result<()> {
        for table in FragmentTable::ALL {
            // Identifier slot: only FragmentTable::sql_name() may supply the table name.
            let sql = format!(
                "UPDATE {} SET is_active = 0
                 WHERE source_relative_path = ?1 AND last_seen_import_run_id != ?2 AND is_active = 1",
                table.sql_name()
            );
            self.conn.execute(&sql, params![relative_path, run_id])?;
        }
        Ok(())
    }

    pub fn reconcile_sidecar(
        &self,
        run_id: &str,
        relative_path: &str,
        table: SidecarTable,
    ) -> Result<()> {
        // Identifier slot: only SidecarTable::sql_name() may supply the table name.
        let sql = format!(
            "UPDATE {} SET is_active = 0
             WHERE source_relative_path = ?1 AND last_seen_import_run_id != ?2 AND is_active = 1",
            table.sql_name()
        );
        self.conn.execute(&sql, params![relative_path, run_id])?;
        Ok(())
    }

    pub fn upsert_asset(&self, row: AssetUpsert<'_>) -> Result<()> {
        self.conn.execute(
            "INSERT INTO assets
             (id, source_key, display_name, source_filename, relative_path, mime_type, size_bytes,
              exists_locally, last_seen_import_run_id, is_active, raw_json)
             VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,1,?10)
             ON CONFLICT(id) DO UPDATE SET
               display_name = excluded.display_name,
               relative_path = excluded.relative_path,
               mime_type = excluded.mime_type,
               size_bytes = excluded.size_bytes,
               exists_locally = excluded.exists_locally,
               last_seen_import_run_id = excluded.last_seen_import_run_id,
               is_active = 1,
               raw_json = excluded.raw_json",
            params![
                row.id,
                row.source_key,
                row.display_name,
                row.source_key,
                row.relative_path,
                row.mime_type,
                row.size_bytes,
                i32::from(row.exists_locally),
                row.run_id,
                row.raw_json,
            ],
        )?;
        Ok(())
    }

    pub fn upsert_message_asset(
        &self,
        message_id: &str,
        asset_id: &str,
        link_source: &str,
        ordinal: i32,
        raw_json: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO message_assets (message_id, asset_id, link_source, ordinal, raw_json)
             VALUES (?1,?2,?3,?4,?5)
             ON CONFLICT(message_id, asset_id, link_source, ordinal) DO UPDATE SET raw_json = excluded.raw_json",
            params![message_id, asset_id, link_source, ordinal, raw_json],
        )?;
        Ok(())
    }

    pub fn upsert_feedback(&self, row: FeedbackUpsert<'_>) -> Result<()> {
        self.conn.execute(
            "INSERT INTO feedback (id, message_id, rating, tags, text, created_at, last_seen_import_run_id, is_active, raw_json)
             VALUES (?1,?2,?3,?4,?5,?6,?7,1,?8)
             ON CONFLICT(id) DO UPDATE SET
               message_id = excluded.message_id,
               rating = excluded.rating,
               tags = excluded.tags,
               text = excluded.text,
               created_at = excluded.created_at,
               last_seen_import_run_id = excluded.last_seen_import_run_id,
               is_active = 1,
               raw_json = excluded.raw_json",
            params![
                row.id,
                row.message_id,
                row.rating,
                row.tags,
                row.text,
                row.created_at,
                row.run_id,
                row.raw_json,
            ],
        )?;
        Ok(())
    }

    pub fn upsert_shared(&self, row: SharedUpsert<'_>) -> Result<()> {
        self.conn.execute(
            "INSERT INTO shared_conversations
             (id, conversation_id, share_id, url, created_at, is_anonymous, last_seen_import_run_id, is_active, raw_json)
             VALUES (?1,?2,?3,?4,?5,?6,?7,1,?8)
             ON CONFLICT(id) DO UPDATE SET
               conversation_id = excluded.conversation_id,
               share_id = excluded.share_id,
               url = excluded.url,
               created_at = excluded.created_at,
               is_anonymous = excluded.is_anonymous,
               last_seen_import_run_id = excluded.last_seen_import_run_id,
               is_active = 1,
               raw_json = excluded.raw_json",
            params![
                row.id,
                row.conversation_id,
                row.share_id,
                row.url,
                row.created_at,
                row.is_anonymous,
                row.run_id,
                row.raw_json,
            ],
        )?;
        Ok(())
    }

    pub fn upsert_library_file(&self, row: LibraryFileUpsert<'_>) -> Result<()> {
        self.conn.execute(
            "INSERT INTO library_files
             (id, file_id, file_name, mime_type, file_size_bytes, sha256_digest, last_seen_import_run_id, is_active, raw_json)
             VALUES (?1,?2,?3,?4,?5,?6,?7,1,?8)
             ON CONFLICT(id) DO UPDATE SET
               file_id = excluded.file_id,
               file_name = excluded.file_name,
               mime_type = excluded.mime_type,
               file_size_bytes = excluded.file_size_bytes,
               sha256_digest = excluded.sha256_digest,
               last_seen_import_run_id = excluded.last_seen_import_run_id,
               is_active = 1,
               raw_json = excluded.raw_json",
            params![
                row.id,
                row.file_id,
                row.file_name,
                row.mime_type,
                row.file_size_bytes,
                row.sha256_digest,
                row.run_id,
                row.raw_json,
            ],
        )?;
        Ok(())
    }

    pub fn reports_dir(db_path: &Path) -> PathBuf {
        db_path
            .parent()
            .map(|p| p.join("reports"))
            .unwrap_or_else(|| PathBuf::from("reports"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::sql_idents::SidecarTable;

    fn seed_import_run(db: &Database, label: &str) -> String {
        db.create_import_run(label, false).unwrap()
    }

    #[test]
    fn reconcile_sidecar_deactivates_stale_feedback() {
        let tmp = tempfile::tempdir().unwrap();
        let db = Database::open(&tmp.path().join("fb.sqlite")).unwrap();
        let run_a = seed_import_run(&db, "a");
        db.upsert_feedback(FeedbackUpsert {
            run_id: &run_a,
            id: "fb-1",
            message_id: Some("msg-1"),
            rating: Some("thumbs_up"),
            tags: None,
            text: None,
            created_at: None,
            raw_json: "{}",
        })
        .unwrap();

        let run_b = seed_import_run(&db, "b");
        db.reconcile_sidecar(&run_b, "message_feedback.json", SidecarTable::Feedback)
            .unwrap();

        let active: i64 = db
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM feedback WHERE id = 'fb-1' AND is_active = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(active, 0);
    }

    #[test]
    fn reconcile_sidecar_deactivates_stale_shared_conversations() {
        let tmp = tempfile::tempdir().unwrap();
        let db = Database::open(&tmp.path().join("shared.sqlite")).unwrap();
        let run_a = seed_import_run(&db, "a");
        db.upsert_shared(SharedUpsert {
            run_id: &run_a,
            id: "sh-1",
            conversation_id: Some("conv-1"),
            share_id: Some("share-1"),
            url: None,
            created_at: None,
            is_anonymous: 0,
            raw_json: "{}",
        })
        .unwrap();

        let run_b = seed_import_run(&db, "b");
        db.reconcile_sidecar(
            &run_b,
            "shared_conversations.json",
            SidecarTable::SharedConversations,
        )
        .unwrap();

        let active: i64 = db
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM shared_conversations WHERE id = 'sh-1' AND is_active = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(active, 0);
    }

    #[test]
    fn reconcile_sidecar_deactivates_stale_library_files() {
        let tmp = tempfile::tempdir().unwrap();
        let db = Database::open(&tmp.path().join("lib.sqlite")).unwrap();
        let run_a = seed_import_run(&db, "a");
        db.upsert_library_file(LibraryFileUpsert {
            run_id: &run_a,
            id: "lib-1",
            file_id: Some("file-1"),
            file_name: Some("doc.pdf"),
            mime_type: None,
            file_size_bytes: None,
            sha256_digest: None,
            raw_json: "{}",
        })
        .unwrap();

        let run_b = seed_import_run(&db, "b");
        db.reconcile_sidecar(&run_b, "library_files.json", SidecarTable::LibraryFiles)
            .unwrap();

        let active: i64 = db
            .connection()
            .query_row(
                "SELECT COUNT(*) FROM library_files WHERE id = 'lib-1' AND is_active = 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(active, 0);
    }

    #[test]
    fn sidecar_table_sql_names_are_closed() {
        assert_eq!(SidecarTable::Feedback.sql_name(), "feedback");
        assert_eq!(
            SidecarTable::SharedConversations.sql_name(),
            "shared_conversations"
        );
        assert_eq!(SidecarTable::LibraryFiles.sql_name(), "library_files");
        assert_eq!(SidecarTable::ALL.len(), 3);
    }
}
