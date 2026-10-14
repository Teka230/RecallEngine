use super::chunking::{chunk_content_block, ChunkerOptions, TextChunk};
use super::embedding::EmbeddingProvider;
use super::store::{SqliteBlobStore, SqliteVecStore, VectorMatch, VectorStore};
use anyhow::Result;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::time::Instant;

/// Benchmark and evaluation report produced by the semantic spike.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpikeReport {
    pub model_id: String,
    pub dimension: usize,
    pub total_content_blocks: usize,
    pub total_chunks: usize,
    pub chunking_duration_us: u128,
    pub embedding_duration_us: u128,
    pub embedding_per_chunk_us: u128,
    pub sqlite_vec_insert_duration_us: u128,
    pub sqlite_blob_insert_duration_us: u128,
    pub sqlite_vec_query_duration_us: u128,
    pub sqlite_blob_query_duration_us: u128,
    pub ranking_top_1_match: bool,
    pub ranking_top_5_overlap_pct: f32,
    pub query_tested: String,
    pub best_match_ic: Option<i64>,
    pub best_match_message_id: String,
    pub best_match_snippet: String,
    pub sqlite_vec_table_bytes: usize,
    pub sqlite_blob_table_bytes: usize,
}

/// Pipeline to run the isolated semantic spike against a RecallEngine SQLite database.
/// An eligible text content block from the canonical store.
#[derive(Debug, Clone)]
pub struct RawContentBlock {
    pub block_id: i64,
    pub message_id: String,
    pub ic: Option<i64>,
    pub text: String,
}

pub struct SemanticSpikeRunner<'a> {
    conn: &'a Connection,
    chunker_options: ChunkerOptions,
}

impl<'a> SemanticSpikeRunner<'a> {
    pub fn new(conn: &'a Connection, chunker_options: ChunkerOptions) -> Self {
        Self {
            conn,
            chunker_options,
        }
    }

    /// Extract all eligible text content blocks from the canonical database.
    pub fn fetch_eligible_content_blocks(&self) -> Result<Vec<RawContentBlock>> {
        let mut stmt = self.conn.prepare(
            "SELECT
                cb.rowid,
                cb.message_id,
                m.ic,
                cb.text_content
            FROM content_blocks cb
            JOIN messages m ON cb.message_id = m.id
            WHERE cb.text_content IS NOT NULL
              AND LENGTH(TRIM(cb.text_content)) > 0
              AND m.role IN ('user', 'assistant')
            ORDER BY m.ic ASC, cb.ordinal ASC",
        )?;

        let rows = stmt.query_map([], |row| {
            Ok(RawContentBlock {
                block_id: row.get(0)?,
                message_id: row.get(1)?,
                ic: row.get(2)?,
                text: row.get(3)?,
            })
        })?;

        let mut blocks = Vec::new();
        for r in rows {
            blocks.push(r?);
        }
        Ok(blocks)
    }

    /// Execute the full spike: chunking, embedding, storage in vec + blob, query comparison & benchmarks.
    pub fn run(
        &self,
        embedder: &mut dyn EmbeddingProvider,
        query: &str,
        top_k: usize,
    ) -> Result<(SpikeReport, Vec<VectorMatch>)> {
        let blocks = self.fetch_eligible_content_blocks()?;
        let total_content_blocks = blocks.len();

        // 1. Chunking
        let start_chunking = Instant::now();
        let mut chunks: Vec<TextChunk> = Vec::new();
        for b in &blocks {
            let produced = chunk_content_block(
                b.block_id,
                &b.message_id,
                b.ic,
                &b.text,
                &self.chunker_options,
            );
            chunks.extend(produced);
        }
        let chunking_duration_us = start_chunking.elapsed().as_micros();
        let total_chunks = chunks.len();

        // 2. Embedding
        let start_embedding = Instant::now();
        let texts: Vec<&str> = chunks.iter().map(|c| c.text.as_str()).collect();
        let embeddings = embedder.embed_passages(&texts)?;
        let embedding_duration_us = start_embedding.elapsed().as_micros();
        let embedding_per_chunk_us = if total_chunks > 0 {
            embedding_duration_us / (total_chunks as u128)
        } else {
            0
        };

        // 3. Storage Setup
        let dim = embedder.dimension();
        let vec_store = SqliteVecStore::new(self.conn, dim);
        let blob_store = SqliteBlobStore::new(self.conn, dim);

        vec_store.create_tables()?;
        blob_store.create_tables()?;

        // 4. Ingestion benchmark: sqlite-vec
        let start_vec_insert = Instant::now();
        for (chunk, vector) in chunks.iter().zip(embeddings.iter()) {
            vec_store.insert_chunk(chunk, vector)?;
        }
        let sqlite_vec_insert_duration_us = start_vec_insert.elapsed().as_micros();

        // 5. Ingestion benchmark: sqlite-blob
        let start_blob_insert = Instant::now();
        for (chunk, vector) in chunks.iter().zip(embeddings.iter()) {
            blob_store.insert_chunk(chunk, vector)?;
        }
        let sqlite_blob_insert_duration_us = start_blob_insert.elapsed().as_micros();

        // 6. Query benchmark: embed query
        let query_vec = embedder.embed_query(query)?;

        // Query: sqlite-vec
        let start_vec_query = Instant::now();
        let vec_results = vec_store.search_knn(&query_vec, top_k)?;
        let sqlite_vec_query_duration_us = start_vec_query.elapsed().as_micros();

        // Query: sqlite-blob
        let start_blob_query = Instant::now();
        let blob_results = blob_store.search_knn(&query_vec, top_k)?;
        let sqlite_blob_query_duration_us = start_blob_query.elapsed().as_micros();

        // 7. Accuracy / Overlap analysis
        let ranking_top_1_match = match (vec_results.first(), blob_results.first()) {
            (Some(v), Some(b)) => {
                v.content_block_id == b.content_block_id && v.chunk_index == b.chunk_index
            }
            (None, None) => true,
            _ => false,
        };

        let vec_top5_keys: std::collections::HashSet<(i64, usize)> = vec_results
            .iter()
            .take(5)
            .map(|r| (r.content_block_id, r.chunk_index))
            .collect();
        let blob_top5_keys: std::collections::HashSet<(i64, usize)> = blob_results
            .iter()
            .take(5)
            .map(|r| (r.content_block_id, r.chunk_index))
            .collect();

        let overlap_count = vec_top5_keys.intersection(&blob_top5_keys).count();
        let ranking_top_5_overlap_pct = if !vec_top5_keys.is_empty() {
            (overlap_count as f32 / vec_top5_keys.len() as f32) * 100.0
        } else {
            100.0
        };

        // 8. Size calculation (approximate bytes)
        let sqlite_blob_table_bytes = total_chunks * (dim * 4 + 128); // 384 * 4 bytes + metadata
        let sqlite_vec_table_bytes = total_chunks * (dim * 4 + 64);

        let best = vec_results.first().cloned();

        let report = SpikeReport {
            model_id: embedder.model_id().to_string(),
            dimension: dim,
            total_content_blocks,
            total_chunks,
            chunking_duration_us,
            embedding_duration_us,
            embedding_per_chunk_us,
            sqlite_vec_insert_duration_us,
            sqlite_blob_insert_duration_us,
            sqlite_vec_query_duration_us,
            sqlite_blob_query_duration_us,
            ranking_top_1_match,
            ranking_top_5_overlap_pct,
            query_tested: query.to_string(),
            best_match_ic: best.as_ref().and_then(|b| b.ic),
            best_match_message_id: best
                .as_ref()
                .map(|b| b.message_id.clone())
                .unwrap_or_default(),
            best_match_snippet: best.as_ref().map(|b| b.text.clone()).unwrap_or_default(),
            sqlite_vec_table_bytes,
            sqlite_blob_table_bytes,
        };

        Ok((report, vec_results))
    }
}
