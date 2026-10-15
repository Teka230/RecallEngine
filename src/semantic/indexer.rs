use super::chunking::{chunk_content_block, ChunkerOptions, TextChunk};
use super::embedding::EmbeddingProvider;
use super::store::{SqliteVecStore, VectorMatch, VectorStore};
use anyhow::Result;
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::time::Instant;

/// Statistics gathered during an indexing run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexingStats {
    /// Total eligible content blocks found in the canonical database.
    pub total_eligible_blocks: usize,
    /// Blocks whose chunk fingerprints matched exactly and required 0 embedding calls.
    pub up_to_date_blocks: usize,
    /// Blocks that were inserted or updated with new embeddings.
    pub indexed_blocks: usize,
    /// Obsolete blocks removed from vector storage.
    pub pruned_blocks: usize,
    /// Total chunks stored in the vector index after this run.
    pub total_chunks: usize,
    /// Total vector embeddings computed during this run.
    pub embeddings_computed: usize,
    /// Elapsed duration of the indexing process in milliseconds.
    pub duration_ms: u128,
}

/// Raw representation of a content block to index.
#[derive(Debug, Clone)]
struct EligibleBlock {
    block_id: String,
    message_id: String,
    ic: Option<i64>,
    text: String,
}

/// Orchestrator for deterministic, idempotent semantic indexing.
pub struct SemanticIndexer<'a> {
    conn: &'a Connection,
    chunker_options: ChunkerOptions,
    batch_size: usize,
    prune: bool,
}

impl<'a> SemanticIndexer<'a> {
    /// Create a new indexer with default chunking parameters and batch size 32.
    pub fn new(conn: &'a Connection) -> Self {
        Self {
            conn,
            chunker_options: ChunkerOptions::default(),
            batch_size: 32,
            prune: true,
        }
    }

    /// Create an indexer with custom chunking options.
    pub fn with_options(conn: &'a Connection, chunker_options: ChunkerOptions) -> Self {
        Self {
            conn,
            chunker_options,
            batch_size: 32,
            prune: true,
        }
    }

    /// Configure batch size for embedding inference.
    pub fn with_batch_size(mut self, batch_size: usize) -> Self {
        self.batch_size = batch_size.max(1);
        self
    }

    /// Configure whether obsolete blocks should be pruned during indexing.
    pub fn with_prune(mut self, prune: bool) -> Self {
        self.prune = prune;
        self
    }

    /// Retrieve eligible text content blocks from messages with user/assistant roles.
    fn fetch_eligible_blocks(&self) -> Result<Vec<EligibleBlock>> {
        let mut stmt = self.conn.prepare(
            "SELECT
                cb.id,
                cb.message_id,
                m.ic,
                cb.text_content
            FROM content_blocks cb
            JOIN messages m ON cb.message_id = m.id
            JOIN conversations c ON m.conversation_id = c.id
            WHERE cb.text_content IS NOT NULL
              AND LENGTH(TRIM(cb.text_content)) > 0
              AND m.role IN ('user', 'assistant')
              AND m.is_active = 1
              AND c.is_active = 1
            ORDER BY m.ic ASC, cb.ordinal ASC",
        )?;

        let rows = stmt.query_map([], |row| {
            Ok(EligibleBlock {
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

    /// Fetch existing chunk fingerprints for a content block.
    fn fetch_existing_fingerprints(&self, block_id: &str) -> Result<Vec<(usize, String)>> {
        let mut stmt = self.conn.prepare(
            "SELECT chunk_index, fingerprint
             FROM semantic_vec_chunks
             WHERE content_block_id = ?1
             ORDER BY chunk_index ASC",
        )?;

        let rows = stmt.query_map(params![block_id], |row| {
            Ok((row.get::<_, i64>(0)? as usize, row.get::<_, String>(1)?))
        })?;

        let mut res = Vec::new();
        for r in rows {
            res.push(r?);
        }
        Ok(res)
    }

    /// Perform a full incremental indexing pass over the canonical database.
    ///
    /// Guaranteed idempotence:
    /// - Evaluates chunk fingerprints before calling inference.
    /// - If fingerprints match, no embedding calls are made.
    /// - Only modified or new blocks trigger embedding computation.
    /// - Obsolete blocks are pruned.
    pub fn index_all(&self, embedder: &mut dyn EmbeddingProvider) -> Result<IndexingStats> {
        let start = Instant::now();
        let dimension = embedder.dimension();
        let model_id = embedder.model_id().to_string();
        let chunker_version = self.chunker_options.version();

        // 1. Ensure storage tables exist
        let store = SqliteVecStore::new(self.conn, dimension);
        store.create_tables()?;
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS semantic_index_meta (
                key TEXT PRIMARY KEY,
                value TEXT NOT NULL,
                updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
            );",
        )?;

        // 2. Fetch eligible blocks
        let eligible = self.fetch_eligible_blocks()?;
        let total_eligible_blocks = eligible.len();
        let mut eligible_block_ids = HashSet::with_capacity(total_eligible_blocks);

        let mut up_to_date_blocks = 0;
        let mut pending_blocks: Vec<(String, Vec<TextChunk>)> = Vec::new();

        // 3. Deterministic chunking & pre-inference fingerprint comparison
        for b in &eligible {
            eligible_block_ids.insert(b.block_id.clone());

            let chunks = chunk_content_block(
                &b.block_id,
                &b.message_id,
                b.ic,
                &b.text,
                &self.chunker_options,
                &model_id,
            );

            if chunks.is_empty() {
                continue;
            }

            let existing_fps = self.fetch_existing_fingerprints(&b.block_id)?;

            let is_up_to_date = existing_fps.len() == chunks.len()
                && existing_fps
                    .iter()
                    .zip(chunks.iter())
                    .all(|((idx, fp), c)| *idx == c.chunk_index && fp == &c.fingerprint);

            if is_up_to_date {
                up_to_date_blocks += 1;
            } else {
                pending_blocks.push((b.block_id.clone(), chunks));
            }
        }

        // 4. Batch inference strictly for pending chunks
        let mut embeddings_computed = 0;
        let mut indexed_blocks = 0;

        if !pending_blocks.is_empty() {
            // Flatten texts for batch embedding
            let mut flat_texts = Vec::new();
            let mut chunk_counts = Vec::with_capacity(pending_blocks.len());

            for (_, chunks) in &pending_blocks {
                chunk_counts.push(chunks.len());
                for c in chunks {
                    flat_texts.push(c.text.as_str());
                }
            }

            let mut all_vectors = Vec::with_capacity(flat_texts.len());
            for chunk_batch in flat_texts.chunks(self.batch_size) {
                let batch_vectors = embedder.embed_passages(chunk_batch)?;
                all_vectors.extend(batch_vectors);
            }
            embeddings_computed = flat_texts.len();

            // Sync each pending block atomically
            let mut vector_offset = 0;
            for ((block_id, chunks), count) in pending_blocks.into_iter().zip(chunk_counts) {
                let block_vectors = &all_vectors[vector_offset..vector_offset + count];
                vector_offset += count;

                store.sync_block_chunks(&block_id, &chunks, block_vectors)?;
                indexed_blocks += 1;
            }
        }

        // 5. Prune obsolete blocks from vector index (if pruning is enabled)
        let mut pruned_blocks = 0;
        if self.prune {
            let mut existing_block_stmt = self
                .conn
                .prepare("SELECT DISTINCT content_block_id FROM semantic_vec_chunks")?;
            let existing_block_ids: Vec<String> = existing_block_stmt
                .query_map([], |row| row.get(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;

            for old_block_id in existing_block_ids {
                if !eligible_block_ids.contains(&old_block_id) {
                    store.sync_block_chunks(&old_block_id, &[], &[])?;
                    pruned_blocks += 1;
                }
            }
        }

        let total_chunks = store.count()?;

        // 6. Record metadata in semantic_index_meta
        self.conn.execute(
            "INSERT INTO semantic_index_meta (key, value, updated_at)
             VALUES ('model_id', ?1, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
             ON CONFLICT(key) DO UPDATE SET value=excluded.value, updated_at=excluded.updated_at",
            params![&model_id],
        )?;
        self.conn.execute(
            "INSERT INTO semantic_index_meta (key, value, updated_at)
             VALUES ('chunker_version', ?1, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
             ON CONFLICT(key) DO UPDATE SET value=excluded.value, updated_at=excluded.updated_at",
            params![&chunker_version],
        )?;
        self.conn.execute(
            "INSERT INTO semantic_index_meta (key, value, updated_at)
             VALUES ('dimension', ?1, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
             ON CONFLICT(key) DO UPDATE SET value=excluded.value, updated_at=excluded.updated_at",
            params![dimension.to_string()],
        )?;
        self.conn.execute(
            "INSERT INTO semantic_index_meta (key, value, updated_at)
             VALUES ('total_indexed_chunks', ?1, strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
             ON CONFLICT(key) DO UPDATE SET value=excluded.value, updated_at=excluded.updated_at",
            params![total_chunks.to_string()],
        )?;
        self.conn.execute(
            "INSERT INTO semantic_index_meta (key, value, updated_at)
             VALUES ('last_indexed_at', strftime('%Y-%m-%dT%H:%M:%fZ', 'now'), strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
             ON CONFLICT(key) DO UPDATE SET value=excluded.value, updated_at=excluded.updated_at",
            [],
        )?;

        let duration_ms = start.elapsed().as_millis();

        Ok(IndexingStats {
            total_eligible_blocks,
            up_to_date_blocks,
            indexed_blocks,
            pruned_blocks,
            total_chunks,
            embeddings_computed,
            duration_ms,
        })
    }

    /// Execute a KNN search against the vector index for a query string.
    pub fn search(
        &self,
        embedder: &mut dyn EmbeddingProvider,
        query: &str,
        k: usize,
    ) -> Result<Vec<VectorMatch>> {
        let store = SqliteVecStore::new(self.conn, embedder.dimension());
        let query_vector = embedder.embed_query(query)?;
        store.search_knn(&query_vector, k)
    }
}
