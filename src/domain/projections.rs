use rusqlite::Connection;

use crate::domain::canonical::{ConversationRecord, MessageCandidate, NodeRecord};
use crate::error::Result;
use crate::storage::sql_idents::CountableTable;

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

pub fn table_count(conn: &Connection, table: CountableTable) -> Result<i64> {
    if table.supports_active_filter() {
        count_active(conn, table).or_else(|_| count_all(conn, table))
    } else {
        count_all(conn, table)
    }
}

pub fn table_count_all(conn: &Connection, table: CountableTable) -> Result<i64> {
    count_all(conn, table)
}

pub fn count_active(conn: &Connection, table: CountableTable) -> Result<i64> {
    // Identifier slot: only CountableTable::sql_name() may supply the table name.
    let sql = format!(
        "SELECT COUNT(*) FROM {} WHERE is_active = 1",
        table.sql_name()
    );
    Ok(conn.query_row(&sql, [], |r| r.get(0))?)
}

pub fn count_all(conn: &Connection, table: CountableTable) -> Result<i64> {
    // Identifier slot: only CountableTable::sql_name() may supply the table name.
    let sql = format!("SELECT COUNT(*) FROM {}", table.sql_name());
    Ok(conn.query_row(&sql, [], |r| r.get(0))?)
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

#[cfg(test)]
mod tests {
    use super::{count_active, count_all, table_count, table_count_all};
    use crate::storage::sql_idents::CountableTable;
    use crate::storage::Database;

    #[test]
    fn countable_table_parity_on_empty_schema() {
        let tmp = tempfile::tempdir().unwrap();
        let db = Database::open(&tmp.path().join("parity.sqlite")).unwrap();
        let conn = db.connection();

        for table in [
            CountableTable::Conversations,
            CountableTable::Nodes,
            CountableTable::Messages,
            CountableTable::Assets,
            CountableTable::Feedback,
            CountableTable::SharedConversations,
            CountableTable::LibraryFiles,
        ] {
            assert_eq!(count_active(conn, table).unwrap(), 0);
            assert_eq!(table_count(conn, table).unwrap(), 0);
        }

        assert_eq!(
            count_all(conn, CountableTable::ContentReferences).unwrap(),
            0
        );
        assert_eq!(
            table_count_all(conn, CountableTable::ContentReferences).unwrap(),
            0
        );
        assert_eq!(
            table_count(conn, CountableTable::ContentReferences).unwrap(),
            0
        );
    }
}
