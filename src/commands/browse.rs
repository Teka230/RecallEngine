use std::path::PathBuf;

use crate::error::Result;

pub fn run(db_path: PathBuf, ic: Option<i64>, conversation: Option<String>) -> Result<()> {
    if matches!(ic, Some(value) if value <= 0) {
        return Err(crate::RecallError::msg("IC must be a positive integer"));
    }
    crate::tui::run(db_path, ic, conversation)
}
