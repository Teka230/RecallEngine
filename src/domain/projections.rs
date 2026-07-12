use rusqlite::Connection;

use crate::domain::canonical::{ConversationRecord, MessageCandidate, NodeRecord};
use crate::error::Result;

pub fn count_branching(conn: &Connection) -> Result<(i64, i64, i64)> {
    let branching: i64 = conn.query_row(
        "SELECT COUNT(*) FROM (
            SELECT parent_id FROM nodes WHERE is_active = 1 AND parent_id IS NOT NULL
            GROUP BY parent_id HAVING COUNT(*) > 1
        )",
        [],
        |r| r.get(0),
    )?;

    let branch_points: i64 = conn.query_row(
        "SELECT COUNT(*) FROM (
            SELECT parent_id FROM nodes WHERE is_active = 1 AND parent_id IS NOT NULL
            GROUP BY parent_id HAVING COUNT(*) > 1
        )",
        [],
        |r| r.get(0),
    )?;

    let max_children: i64 = conn.query_row(
        "SELECT COALESCE(MAX(cnt), 0) FROM (
            SELECT COUNT(*) AS cnt FROM nodes WHERE is_active = 1 AND parent_id IS NOT NULL
            GROUP BY parent_id
        )",
        [],
        |r| r.get(0),
    )?;

    Ok((branching, branch_points, max_children))
}

pub fn count_by_role(conn: &Connection, role: &str) -> Result<i64> {
    let c: i64 = conn.query_row(
        "SELECT COUNT(*) FROM messages WHERE is_active = 1 AND role = ?1",
        [role],
        |r| r.get(0),
    )?;
    Ok(c)
}

pub fn table_count(conn: &Connection, table: &str) -> Result<i64> {
    let sql = format!("SELECT COUNT(*) FROM {table} WHERE is_active = 1");
    let c: i64 = conn.query_row(&sql, [], |r| r.get(0)).or_else(|_| {
        let sql_all = format!("SELECT COUNT(*) FROM {table}");
        conn.query_row(&sql_all, [], |r| r.get(0))
    })?;
    Ok(c)
}

pub fn table_count_all(conn: &Connection, table: &str) -> Result<i64> {
    let sql = format!("SELECT COUNT(*) FROM {table}");
    let c: i64 = conn.query_row(&sql, [], |r| r.get(0))?;
    Ok(c)
}

pub fn active_conversations_with_branches(conn: &Connection) -> Result<i64> {
    let c: i64 = conn.query_row(
        "SELECT COUNT(DISTINCT conversation_id) FROM (
            SELECT n.conversation_id, n.parent_id, COUNT(*) AS c
            FROM nodes n WHERE n.is_active = 1 AND n.parent_id IS NOT NULL
            GROUP BY n.conversation_id, n.parent_id HAVING c > 1
        )",
        [],
        |r| r.get(0),
    )?;
    Ok(c)
}

#[allow(dead_code)]
pub fn _unused_types(_: ConversationRecord, _: NodeRecord, _: MessageCandidate) {}
