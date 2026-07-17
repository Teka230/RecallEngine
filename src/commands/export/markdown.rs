#![allow(clippy::ptr_arg)]
use crate::error::{RecallError, Result};
use crate::export::markdown::options::MarkdownRenderOptions;
use crate::export::markdown::renderer::MarkdownRenderer;
use crate::output::json::JsonEnvelope;
use crate::repository::sqlite::SqliteRepository;
use rusqlite::Connection;
use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;

pub fn run_markdown(db_path: PathBuf, ic: i64, out_path: PathBuf) -> Result<()> {
    match run_markdown_inner(&db_path, ic, &out_path) {
        Ok(_) => {
            if out_path.to_string_lossy() != "-" {
                let env = JsonEnvelope::new(serde_json::json!({
                    "status": "success",
                    "ic": ic,
                    "file": out_path
                }));
                let json_str = serde_json::to_string(&env).unwrap_or_else(|_| "{}".to_string());
                println!("{}", json_str);
            }
            Ok(())
        }
        Err(e) => {
            // Note: In real app, we'd map this to JsonEnvelope::error but here we just return it
            Err(e)
        }
    }
}

fn run_markdown_inner(db_path: &PathBuf, ic: i64, out_path: &PathBuf) -> Result<()> {
    let conn = Connection::open(db_path)?;
    let repo = SqliteRepository::new(&conn);

    let conv = repo
        .resolve_ic(ic)?
        .ok_or_else(|| RecallError::msg(format!("Conversation not found for IC {}", ic)))?;

    let messages = repo.get_messages(&conv.id)?;

    let mut messages_with_blocks = Vec::new();
    for msg in messages {
        let blocks = repo.get_content_blocks(&msg.id)?;
        messages_with_blocks.push((msg, blocks));
    }

    let options = MarkdownRenderOptions::default();
    let renderer = MarkdownRenderer::new(&options);

    let markdown_content = renderer.render(&conv, &messages_with_blocks);

    if out_path.to_string_lossy() == "-" {
        print!("{}", markdown_content);
    } else {
        write_atomically(out_path, &markdown_content)?;
    }

    Ok(())
}

fn write_atomically(path: &Path, content: &str) -> Result<()> {
    let tmp_path = path.with_extension("tmp");
    {
        let mut file = File::create(&tmp_path).map_err(RecallError::Io)?;
        file.write_all(content.as_bytes())
            .map_err(RecallError::Io)?;
        file.sync_all().map_err(RecallError::Io)?;
    }
    std::fs::rename(&tmp_path, path).map_err(RecallError::Io)?;
    Ok(())
}
