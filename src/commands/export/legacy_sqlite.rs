use std::path::PathBuf;

use crate::error::Result;
use crate::export::legacy;
use crate::export::legacy::LegacyExportScope;
use crate::storage::Database;

pub fn run_legacy_sqlite(
    db_path: PathBuf,
    output: PathBuf,
    scope: LegacyExportScope,
) -> Result<()> {
    let db = Database::open(&db_path)?;
    legacy::export_legacy_sqlite_with_scope(db.connection(), &output, scope)?;
    Ok(())
}
