use crate::error::{RecallError, Result};
use crate::output::json::JsonEnvelope;
use crate::semantic::embedding::{
    is_e5_small_cached, EmbeddingProvider, FastEmbedProvider, OfflineMode,
};
use crate::semantic::indexer::SemanticIndexer;
use crate::storage::Database;
use std::path::PathBuf;
use std::time::Instant;

#[derive(Debug, Clone)]
pub struct EmbedOptions {
    pub db_path: PathBuf,
    pub batch_size: usize,
    pub prune: bool,
    pub offline_mode: OfflineMode,
    pub cache_dir: Option<PathBuf>,
    pub as_json: bool,
}

pub fn run_embed(options: EmbedOptions) -> Result<()> {
    if !options.db_path.exists() {
        return Err(RecallError::msg(format!(
            "Database file not found: {}",
            options.db_path.display()
        )));
    }

    let start = Instant::now();

    // 1. Validate offline cache if in StrictOffline mode
    if options.offline_mode == OfflineMode::StrictOffline
        && !is_e5_small_cached(options.cache_dir.as_deref())
    {
        return Err(RecallError::msg(
            "Model cache for 'intfloat/multilingual-e5-small' not found.\n\
             To operate in strict offline mode, pre-populate the cache directory or run with --allow-download.",
        ));
    }

    // 2. Open database (applies migration 004 and registers sqlite-vec)
    let db = Database::open(&options.db_path)?;

    // 3. Initialize FastEmbed provider
    let mut embedder = match options.offline_mode {
        OfflineMode::StrictOffline => FastEmbedProvider::try_new_offline(options.cache_dir)?,
        OfflineMode::AllowDownload => FastEmbedProvider::try_new(options.cache_dir)?,
    };

    let model_id = embedder.model_id().to_string();

    if !options.as_json {
        println!(
            "Indexing semantic embeddings for '{}' (model: {}, batch size: {})...",
            options.db_path.display(),
            model_id,
            options.batch_size
        );
    }

    // 4. Run indexer
    let indexer = SemanticIndexer::new(db.connection())
        .with_batch_size(options.batch_size)
        .with_prune(options.prune);

    let stats = indexer.index_all(&mut embedder)?;
    let total_elapsed = start.elapsed();

    // 5. Output results
    if options.as_json {
        let env = JsonEnvelope::new(stats);
        let json_str = serde_json::to_string_pretty(&env)
            .map_err(|e| RecallError::msg(format!("Failed to serialize stats: {e}")))?;
        println!("{json_str}");
    } else {
        println!();
        if stats.indexed_blocks == 0 && stats.pruned_blocks == 0 {
            println!("Index is already up to date ({:.1?}).", total_elapsed);
            println!("  - Eligible blocks : {}", stats.total_eligible_blocks);
            println!(
                "  - Up-to-date      : {} (0 recalculations)",
                stats.up_to_date_blocks
            );
            println!("  - Total chunks    : {}", stats.total_chunks);
        } else {
            println!(
                "Semantic index updated successfully in {:.1?}.",
                total_elapsed
            );
            println!("  - Eligible blocks     : {}", stats.total_eligible_blocks);
            println!("  - Indexed blocks      : {}", stats.indexed_blocks);
            println!("  - Up-to-date blocks   : {}", stats.up_to_date_blocks);
            println!("  - Pruned blocks       : {}", stats.pruned_blocks);
            println!("  - Embeddings computed : {}", stats.embeddings_computed);
            println!("  - Total index chunks  : {}", stats.total_chunks);
            if stats.embeddings_computed > 0 && stats.duration_ms > 0 {
                let chunks_per_sec =
                    (stats.embeddings_computed as f64) / (stats.duration_ms as f64 / 1000.0);
                println!("  - Ingestion speed     : {:.1} chunks/s", chunks_per_sec);
            }
        }
    }

    Ok(())
}
