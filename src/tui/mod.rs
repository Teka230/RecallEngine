pub mod actions;
pub mod app;
pub mod event;
pub mod overlays;
pub mod render;
pub mod state;
pub mod text;

#[cfg(test)]
mod tests;

use crate::Result;
use std::path::PathBuf;

pub fn run(db_path: PathBuf, ic: Option<i64>, conversation: Option<String>) -> Result<()> {
    app::run(db_path, ic, conversation)
}
