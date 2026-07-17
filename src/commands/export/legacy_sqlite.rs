use std::path::PathBuf;

use crate::error::Result;
use crate::export::legacy;
use crate::storage::Database;

pub fn run_legacy_sqlite(db_path: PathBuf, output: PathBuf) -> Result<()> {
    let db = Database::open(&db_path)?;
    legacy::export_legacy_sqlite(db.connection(), &output)?;
    Ok(())
}
