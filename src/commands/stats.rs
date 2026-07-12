use std::path::PathBuf;

use serde_json::json;

use crate::error::Result;
use crate::storage::Database;

pub fn run(db_path: PathBuf, as_json: bool) -> Result<()> {
    let db = Database::open(&db_path)?;
    let conn = db.connection();

    let stats = json!({
        "conversations": count_active(conn, "conversations")?,
        "nodes": count_active(conn, "nodes")?,
        "messages": count_active(conn, "messages")?,
        "assistant_messages": count_role(conn, "assistant")?,
        "user_messages": count_role(conn, "user")?,
        "branching_conversations": branching_conversations(conn)?,
        "branch_points": branch_points(conn)?,
        "max_children": max_children(conn)?,
        "mapped_assets": count_all(conn, "assets")?,
        "attachments": count_message_assets(conn)?,
        "content_references": count_all(conn, "content_references")?,
        "feedback": count_active(conn, "feedback")?,
        "shared_conversations": count_active(conn, "shared_conversations")?,
        "library_files": count_active(conn, "library_files")?,
    });

    if as_json {
        println!("{}", serde_json::to_string_pretty(&stats)?);
    } else {
        for (k, v) in stats.as_object().unwrap() {
            println!("{k}: {v}");
        }
    }
    Ok(())
}

fn count_active(conn: &rusqlite::Connection, table: &str) -> Result<i64> {
    let sql = format!("SELECT COUNT(*) FROM {table} WHERE is_active = 1");
    Ok(conn.query_row(&sql, [], |r| r.get(0))?)
}

fn count_all(conn: &rusqlite::Connection, table: &str) -> Result<i64> {
    let sql = format!("SELECT COUNT(*) FROM {table}");
    Ok(conn.query_row(&sql, [], |r| r.get(0))?)
}

fn count_role(conn: &rusqlite::Connection, role: &str) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM messages WHERE is_active = 1 AND role = ?1",
        [role],
        |r| r.get(0),
    )?)
}

fn branching_conversations(conn: &rusqlite::Connection) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(DISTINCT conversation_id) FROM (
            SELECT conversation_id, parent_id, COUNT(*) c FROM nodes
            WHERE is_active = 1 AND parent_id IS NOT NULL
            GROUP BY conversation_id, parent_id HAVING c > 1
        )",
        [],
        |r| r.get(0),
    )?)
}

fn branch_points(conn: &rusqlite::Connection) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM (
            SELECT parent_id FROM nodes WHERE is_active = 1 AND parent_id IS NOT NULL
            GROUP BY parent_id HAVING COUNT(*) > 1
        )",
        [],
        |r| r.get(0),
    )?)
}

fn max_children(conn: &rusqlite::Connection) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COALESCE(MAX(cnt),0) FROM (
            SELECT COUNT(*) cnt FROM nodes
            WHERE is_active = 1 AND parent_id IS NOT NULL AND parent_id != 'client-created-root'
            GROUP BY parent_id
        )",
        [],
        |r| r.get(0),
    )?)
}

fn count_message_assets(conn: &rusqlite::Connection) -> Result<i64> {
    Ok(conn.query_row(
        "SELECT COUNT(*) FROM message_assets WHERE link_source = 'metadata_attachment'",
        [],
        |r| r.get(0),
    )?)
}
