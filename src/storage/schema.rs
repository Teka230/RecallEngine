pub const MIGRATION_001: &str = include_str!("migrations/001_initial.sql");
pub const MIGRATION_002: &str = include_str!("migrations/002_conversations_fts.sql");

pub fn apply_migrations(conn: &rusqlite::Connection) -> crate::Result<()> {
    conn.execute_batch(MIGRATION_001)?;
    conn.execute_batch(MIGRATION_002)?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS recall_engine_migrations (
            id TEXT PRIMARY KEY
        );",
    )?;
    let fts_backfill_applied: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM recall_engine_migrations WHERE id = 'fts5-content-blocks-v1')",
        [],
        |row| row.get(0),
    )?;
    if !fts_backfill_applied {
        conn.execute(
            "INSERT INTO content_blocks_fts(content_blocks_fts) VALUES ('rebuild')",
            [],
        )?;
        conn.execute(
            "INSERT INTO recall_engine_migrations (id) VALUES ('fts5-content-blocks-v1')",
            [],
        )?;
    }
    let conv_fts_backfill_applied: bool = conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM recall_engine_migrations WHERE id = 'fts5-conversations-v1')",
        [],
        |row| row.get(0),
    )?;
    if !conv_fts_backfill_applied {
        conn.execute(
            "INSERT INTO conversations_fts(conversations_fts) VALUES ('rebuild')",
            [],
        )?;
        conn.execute(
            "INSERT INTO recall_engine_migrations (id) VALUES ('fts5-conversations-v1')",
            [],
        )?;
    }
    Ok(())
}
