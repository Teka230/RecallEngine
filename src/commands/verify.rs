use std::path::PathBuf;

use tracing::info;

use crate::error::{RecallError, Result};
use crate::storage::Database;

pub fn run(db_path: PathBuf) -> Result<()> {
    let db = Database::open(&db_path)?;
    let conn = db.connection();
    let mut errors = Vec::new();

    // FK orphans: messages without nodes
    let orphan_messages: i64 = conn.query_row(
        "SELECT COUNT(*) FROM messages m
         LEFT JOIN nodes n ON n.id = m.node_id
         WHERE m.is_active = 1 AND n.id IS NULL",
        [],
        |r| r.get(0),
    )?;
    if orphan_messages > 0 {
        errors.push(format!("{orphan_messages} active messages without node"));
    }

    // Duplicate IC among active
    let dup_ic: i64 = conn.query_row(
        "SELECT COUNT(*) FROM (
            SELECT ic FROM messages WHERE is_active = 1 GROUP BY ic HAVING COUNT(*) > 1
        )",
        [],
        |r| r.get(0),
    )?;
    if dup_ic > 0 {
        errors.push(format!(
            "{dup_ic} duplicate IC values among active messages"
        ));
    }

    // Referenced assets missing locally
    let missing_assets: i64 = conn.query_row(
        "SELECT COUNT(*) FROM assets WHERE is_active = 1 AND exists_locally = 0",
        [],
        |r| r.get(0),
    )?;
    if missing_assets > 0 {
        info!("{missing_assets} assets referenced but not present locally (warning)");
    }

    // Inactive messages should still have unique IC
    let inactive_ic_reuse: i64 = conn.query_row(
        "SELECT COUNT(*) FROM (
            SELECT ic FROM messages GROUP BY ic HAVING COUNT(*) > 1
        )",
        [],
        |r| r.get(0),
    )?;
    if inactive_ic_reuse > 0 {
        errors.push(format!(
            "{inactive_ic_reuse} IC values reused across messages"
        ));
    }

    if !errors.is_empty() {
        for e in &errors {
            eprintln!("verify error: {e}");
        }
        return Err(RecallError::VerifyFailed);
    }

    info!("verify OK");
    Ok(())
}
