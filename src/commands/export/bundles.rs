use std::collections::HashMap;
use std::path::PathBuf;

use crate::cli::BundleFormat;
use crate::error::Result;
use crate::export::bundles::planner::BundlePlanner;
use crate::export::bundles::profile::BundleProfile;
use crate::export::bundles::writer::BundleWriter;
use crate::export::markdown::options::MarkdownRenderOptions;
use crate::export::markdown::renderer::MarkdownRenderer;
use crate::repository::SqliteRepository;

pub fn run_bundle(
    db_path: PathBuf,
    out_path: PathBuf,
    profile_name: String,
    format: BundleFormat,
    force: bool,
) -> Result<()> {
    tracing::info!(
        "Starting bundle export to {:?} with profile '{}'",
        out_path,
        profile_name
    );

    let conn = rusqlite::Connection::open(&db_path)?;
    let repo = SqliteRepository::new(&conn);
    let profile = BundleProfile::from_name(&profile_name).ok_or_else(|| {
        crate::error::RecallError::msg(format!("Unknown profile: {}", profile_name))
    })?;

    // Use default markdown options
    let options = MarkdownRenderOptions::default();
    let renderer = MarkdownRenderer::new(&options);

    // Fetch all active conversations and their messages
    let mut stmt =
        conn.prepare("SELECT id FROM conversations WHERE is_active = 1 ORDER BY id ASC")?;
    let mut rows = stmt.query([])?;

    let mut conv_ids = Vec::new();
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        conv_ids.push(id);
    }

    let mut all_conversations = Vec::new();
    let mut conversations_map = HashMap::new();

    tracing::info!(
        "Found {} active conversations, loading data...",
        conv_ids.len()
    );

    for id in conv_ids {
        if let Some(conv) = repo.get_conversation(&id)? {
            let messages = repo.get_messages(&conv.id)?;
            let mut messages_with_blocks = Vec::new();
            for msg in messages {
                let blocks = repo.get_content_blocks(&msg.id)?;
                messages_with_blocks.push((msg, blocks));
            }
            all_conversations.push((conv.clone(), messages_with_blocks.clone()));
            conversations_map.insert(id, (conv, messages_with_blocks));
        }
    }

    // Pass data to planner
    tracing::info!("Planning bundle generation...");
    let planner = BundlePlanner::new(profile, &renderer);
    let plan = planner
        .plan(all_conversations)
        .map_err(crate::error::RecallError::msg)?;

    // Create a map that matches what writer expects
    let mut ref_map = HashMap::new();
    for (id, tuple) in &conversations_map {
        ref_map.insert(id.clone(), tuple);
    }

    let writer = BundleWriter::new(&renderer);
    match format {
        BundleFormat::Zip => {
            tracing::info!("Writing zip to {:?}...", out_path);
            writer.write_zip(&plan, &ref_map, &out_path, force)?;
        }
        BundleFormat::Directory => {
            tracing::info!("Writing directory to {:?}...", out_path);
            writer.write_directory(&plan, &ref_map, &out_path, force)?;
        }
    }

    tracing::info!("Bundle export completed successfully.");
    Ok(())
}
