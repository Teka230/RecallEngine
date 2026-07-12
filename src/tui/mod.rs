mod app;

use std::path::PathBuf;

use crate::error::Result;

pub fn run(db_path: PathBuf, ic: Option<i64>, conversation: Option<String>) -> Result<()> {
    app::run(db_path, ic, conversation)
}
