use super::chunking::TextChunk;
use anyhow::{anyhow, Result};
use rusqlite::ffi::sqlite3_auto_extension;
use rusqlite::{params, Connection};
use sqlite_vec::sqlite3_vec_init;
use std::sync::Once;

static INIT_SQLITE_VEC: Once = Once::new();

/// Initialize the sqlite-vec extension globally for rusqlite connections.
#[allow(clippy::missing_transmute_annotations)]
pub fn ensure_sqlite_vec_registered() {
    INIT_SQLITE_VEC.call_once(|| unsafe {
        sqlite3_auto_extension(Some(std::mem::transmute(sqlite3_vec_init as *const ())));
    });
}

/// Helper to cast a slice of f32 to raw little-endian bytes.
pub fn f32_slice_to_bytes(slice: &[f32]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(slice.as_ptr() as *const u8, std::mem::size_of_val(slice)) }
}

/// Helper to parse raw bytes into a Vec<f32>.
#[allow(clippy::manual_is_multiple_of, clippy::chunks_exact_to_as_chunks)]
pub fn bytes_to_f32_vec(bytes: &[u8]) -> Result<Vec<f32>> {
    if !bytes.len().is_multiple_of(std::mem::size_of::<f32>()) {
        return Err(anyhow!(
            "Invalid byte length for f32 slice: {}",
            bytes.len()
        ));
    }
    let count = bytes.len() / std::mem::size_of::<f32>();
    let mut vec = Vec::with_capacity(count);
    for chunk in bytes.chunks_exact(std::mem::size_of::<f32>()) {
        let arr: [u8; 4] = chunk.try_into().unwrap();
        vec.push(f32::from_le_bytes(arr));
    }
    Ok(vec)
}

/// Compute cosine distance between two normalized vectors: 1.0 - dot_product.
pub fn cosine_distance(a: &[f32], b: &[f32]) -> f32 {
    let mut dot = 0.0f32;
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
    }
    (1.0 - dot).max(0.0)
}

/// Status of chunk synchronization for a content block.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncStatus {
    /// Chunks were identical; no database write performed.
    UpToDate,
    /// Chunks were inserted fresh for this block.
    Inserted { count: usize },
    /// Existing chunks were updated/replaced atomically.
    Replaced { old_count: usize, new_count: usize },
}

/// A matched chunk from vector retrieval.
#[derive(Debug, Clone, PartialEq)]
pub struct VectorMatch {
    pub chunk_id: i64,
    pub content_block_id: String,
    pub message_id: String,
    pub ic: Option<i64>,
    pub chunk_index: usize,
    pub chunk_hash: String,
    pub text: String,
    pub distance: f32,
    pub similarity: f32,
}

/// Trait defining a vector store for chunks.
pub trait VectorStore {
    fn create_tables(&self) -> Result<()>;
    fn insert_chunk(&self, chunk: &TextChunk, vector: &[f32]) -> Result<i64>;
    fn sync_block_chunks(
        &self,
        content_block_id: &str,
        chunks: &[TextChunk],
        vectors: &[Vec<f32>],
    ) -> Result<SyncStatus>;
    fn search_knn(&self, query_vector: &[f32], k: usize) -> Result<Vec<VectorMatch>>;
    fn count(&self) -> Result<usize>;
}

/// Vector store implementation using sqlite-vec virtual table (vec0).
pub struct SqliteVecStore<'a> {
    conn: &'a Connection,
    dimension: usize,
}

impl<'a> SqliteVecStore<'a> {
    pub fn new(conn: &'a Connection, dimension: usize) -> Self {
        ensure_sqlite_vec_registered();
        Self { conn, dimension }
    }
}

impl<'a> VectorStore for SqliteVecStore<'a> {
    fn create_tables(&self) -> Result<()> {
        self.conn.execute_batch(&format!(
            "
            CREATE TABLE IF NOT EXISTS semantic_vec_chunks (
                chunk_id INTEGER PRIMARY KEY AUTOINCREMENT,
                content_block_id TEXT NOT NULL,
                message_id TEXT NOT NULL,
                ic INTEGER,
                chunk_index INTEGER NOT NULL,
                total_chunks INTEGER NOT NULL,
                chunk_hash TEXT NOT NULL,
                text TEXT NOT NULL,
                UNIQUE (content_block_id, chunk_index)
            );
            CREATE INDEX IF NOT EXISTS idx_vec_chunks_block ON semantic_vec_chunks(content_block_id);
            CREATE INDEX IF NOT EXISTS idx_vec_chunks_hash ON semantic_vec_chunks(chunk_hash);

            CREATE VIRTUAL TABLE IF NOT EXISTS semantic_vec_items USING vec0(
                chunk_id INTEGER PRIMARY KEY,
                embedding float[{}] distance_metric=cosine
            );
            ",
            self.dimension
        ))?;
        Ok(())
    }

    fn insert_chunk(&self, chunk: &TextChunk, vector: &[f32]) -> Result<i64> {
        if vector.len() != self.dimension {
            return Err(anyhow!(
                "Vector dimension mismatch: expected {}, got {}",
                self.dimension,
                vector.len()
            ));
        }

        self.conn.execute(
            "INSERT INTO semantic_vec_chunks (
                content_block_id, message_id, ic, chunk_index, total_chunks, chunk_hash, text
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                &chunk.content_block_id,
                &chunk.message_id,
                chunk.ic,
                chunk.chunk_index as i64,
                chunk.total_chunks as i64,
                &chunk.chunk_hash,
                &chunk.text,
            ],
        )?;

        let chunk_id = self.conn.last_insert_rowid();
        let vector_bytes = f32_slice_to_bytes(vector);

        self.conn.execute(
            "INSERT INTO semantic_vec_items (chunk_id, embedding) VALUES (?1, ?2)",
            params![chunk_id, vector_bytes],
        )?;

        Ok(chunk_id)
    }

    fn sync_block_chunks(
        &self,
        content_block_id: &str,
        chunks: &[TextChunk],
        vectors: &[Vec<f32>],
    ) -> Result<SyncStatus> {
        if chunks.len() != vectors.len() {
            return Err(anyhow!(
                "Chunks count ({}) does not match vectors count ({})",
                chunks.len(),
                vectors.len()
            ));
        }
        for v in vectors {
            if v.len() != self.dimension {
                return Err(anyhow!(
                    "Vector dimension mismatch: expected {}, got {}",
                    self.dimension,
                    v.len()
                ));
            }
        }

        self.conn.execute_batch("SAVEPOINT sync_vec_block")?;

        let res = (|| -> Result<SyncStatus> {
            let mut stmt = self.conn.prepare(
                "SELECT chunk_id, chunk_index, chunk_hash
                 FROM semantic_vec_chunks
                 WHERE content_block_id = ?1
                 ORDER BY chunk_index ASC",
            )?;

            let existing: Vec<(i64, usize, String)> = stmt
                .query_map(params![content_block_id], |r| {
                    Ok((r.get(0)?, r.get::<_, i64>(1)? as usize, r.get(2)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;

            // Check if existing chunks exactly match new chunks
            if existing.len() == chunks.len()
                && existing
                    .iter()
                    .zip(chunks.iter())
                    .all(|((_, idx, hash), c)| *idx == c.chunk_index && hash == &c.chunk_hash)
            {
                return Ok(SyncStatus::UpToDate);
            }

            let old_count = existing.len();

            // Delete stale items from virtual and metadata tables
            if !existing.is_empty() {
                for (chunk_id, _, _) in &existing {
                    self.conn.execute(
                        "DELETE FROM semantic_vec_items WHERE chunk_id = ?1",
                        params![chunk_id],
                    )?;
                }
                self.conn.execute(
                    "DELETE FROM semantic_vec_chunks WHERE content_block_id = ?1",
                    params![content_block_id],
                )?;
            }

            // Insert new chunks
            for (chunk, vector) in chunks.iter().zip(vectors.iter()) {
                self.insert_chunk(chunk, vector)?;
            }

            let new_count = chunks.len();
            let status = if old_count == 0 {
                SyncStatus::Inserted { count: new_count }
            } else {
                SyncStatus::Replaced {
                    old_count,
                    new_count,
                }
            };

            Ok(status)
        })();

        match res {
            Ok(status) => {
                self.conn.execute_batch("RELEASE sync_vec_block")?;
                Ok(status)
            }
            Err(e) => {
                let _ = self
                    .conn
                    .execute_batch("ROLLBACK TO sync_vec_block; RELEASE sync_vec_block;");
                Err(e)
            }
        }
    }

    fn search_knn(&self, query_vector: &[f32], k: usize) -> Result<Vec<VectorMatch>> {
        if query_vector.len() != self.dimension {
            return Err(anyhow!(
                "Query dimension mismatch: expected {}, got {}",
                self.dimension,
                query_vector.len()
            ));
        }

        let query_bytes = f32_slice_to_bytes(query_vector);
        let mut stmt = self.conn.prepare(
            "SELECT
                c.chunk_id,
                c.content_block_id,
                c.message_id,
                c.ic,
                c.chunk_index,
                c.chunk_hash,
                c.text,
                v.distance
            FROM semantic_vec_items v
            JOIN semantic_vec_chunks c ON v.chunk_id = c.chunk_id
            WHERE v.embedding MATCH ?1 AND k = ?2
            ORDER BY v.distance ASC",
        )?;

        let rows = stmt.query_map(params![query_bytes, k as i64], |row| {
            let distance: f32 = row.get(7)?;
            Ok(VectorMatch {
                chunk_id: row.get(0)?,
                content_block_id: row.get(1)?,
                message_id: row.get(2)?,
                ic: row.get(3)?,
                chunk_index: row.get::<_, i64>(4)? as usize,
                chunk_hash: row.get(5)?,
                text: row.get(6)?,
                distance,
                similarity: 1.0 / (1.0 + distance),
            })
        })?;

        let mut matches = Vec::new();
        for m in rows {
            matches.push(m?);
        }
        Ok(matches)
    }

    fn count(&self) -> Result<usize> {
        let count: i64 =
            self.conn
                .query_row("SELECT COUNT(*) FROM semantic_vec_chunks", [], |r| r.get(0))?;
        Ok(count as usize)
    }
}

/// Baseline vector store using standard SQLite table and BLOB storage.
pub struct SqliteBlobStore<'a> {
    conn: &'a Connection,
    dimension: usize,
}

impl<'a> SqliteBlobStore<'a> {
    pub fn new(conn: &'a Connection, dimension: usize) -> Self {
        Self { conn, dimension }
    }
}

impl<'a> VectorStore for SqliteBlobStore<'a> {
    fn create_tables(&self) -> Result<()> {
        self.conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS semantic_blob_chunks (
                chunk_id INTEGER PRIMARY KEY AUTOINCREMENT,
                content_block_id TEXT NOT NULL,
                message_id TEXT NOT NULL,
                ic INTEGER,
                chunk_index INTEGER NOT NULL,
                total_chunks INTEGER NOT NULL,
                chunk_hash TEXT NOT NULL,
                text TEXT NOT NULL,
                embedding BLOB NOT NULL,
                UNIQUE (content_block_id, chunk_index)
            );
            CREATE INDEX IF NOT EXISTS idx_blob_chunks_block ON semantic_blob_chunks(content_block_id);
            CREATE INDEX IF NOT EXISTS idx_blob_chunks_hash ON semantic_blob_chunks(chunk_hash);
            ",
        )?;
        Ok(())
    }

    fn insert_chunk(&self, chunk: &TextChunk, vector: &[f32]) -> Result<i64> {
        if vector.len() != self.dimension {
            return Err(anyhow!(
                "Vector dimension mismatch: expected {}, got {}",
                self.dimension,
                vector.len()
            ));
        }

        let vector_bytes = f32_slice_to_bytes(vector);
        self.conn.execute(
            "INSERT INTO semantic_blob_chunks (
                content_block_id, message_id, ic, chunk_index, total_chunks, chunk_hash, text, embedding
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                &chunk.content_block_id,
                &chunk.message_id,
                chunk.ic,
                chunk.chunk_index as i64,
                chunk.total_chunks as i64,
                &chunk.chunk_hash,
                &chunk.text,
                vector_bytes,
            ],
        )?;

        Ok(self.conn.last_insert_rowid())
    }

    fn sync_block_chunks(
        &self,
        content_block_id: &str,
        chunks: &[TextChunk],
        vectors: &[Vec<f32>],
    ) -> Result<SyncStatus> {
        if chunks.len() != vectors.len() {
            return Err(anyhow!(
                "Chunks count ({}) does not match vectors count ({})",
                chunks.len(),
                vectors.len()
            ));
        }
        for v in vectors {
            if v.len() != self.dimension {
                return Err(anyhow!(
                    "Vector dimension mismatch: expected {}, got {}",
                    self.dimension,
                    v.len()
                ));
            }
        }

        self.conn.execute_batch("SAVEPOINT sync_blob_block")?;

        let res = (|| -> Result<SyncStatus> {
            let mut stmt = self.conn.prepare(
                "SELECT chunk_id, chunk_index, chunk_hash
                 FROM semantic_blob_chunks
                 WHERE content_block_id = ?1
                 ORDER BY chunk_index ASC",
            )?;

            let existing: Vec<(i64, usize, String)> = stmt
                .query_map(params![content_block_id], |r| {
                    Ok((r.get(0)?, r.get::<_, i64>(1)? as usize, r.get(2)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;

            // Check if existing chunks exactly match new chunks
            if existing.len() == chunks.len()
                && existing
                    .iter()
                    .zip(chunks.iter())
                    .all(|((_, idx, hash), c)| *idx == c.chunk_index && hash == &c.chunk_hash)
            {
                return Ok(SyncStatus::UpToDate);
            }

            let old_count = existing.len();

            if !existing.is_empty() {
                self.conn.execute(
                    "DELETE FROM semantic_blob_chunks WHERE content_block_id = ?1",
                    params![content_block_id],
                )?;
            }

            for (chunk, vector) in chunks.iter().zip(vectors.iter()) {
                self.insert_chunk(chunk, vector)?;
            }

            let new_count = chunks.len();
            let status = if old_count == 0 {
                SyncStatus::Inserted { count: new_count }
            } else {
                SyncStatus::Replaced {
                    old_count,
                    new_count,
                }
            };

            Ok(status)
        })();

        match res {
            Ok(status) => {
                self.conn.execute_batch("RELEASE sync_blob_block")?;
                Ok(status)
            }
            Err(e) => {
                let _ = self
                    .conn
                    .execute_batch("ROLLBACK TO sync_blob_block; RELEASE sync_blob_block;");
                Err(e)
            }
        }
    }

    fn search_knn(&self, query_vector: &[f32], k: usize) -> Result<Vec<VectorMatch>> {
        if query_vector.len() != self.dimension {
            return Err(anyhow!(
                "Query dimension mismatch: expected {}, got {}",
                self.dimension,
                query_vector.len()
            ));
        }

        let mut stmt = self.conn.prepare(
            "SELECT
                chunk_id,
                content_block_id,
                message_id,
                ic,
                chunk_index,
                chunk_hash,
                text,
                embedding
            FROM semantic_blob_chunks",
        )?;

        let mut all_scored = Vec::new();
        let mut rows = stmt.query([])?;

        while let Some(row) = rows.next()? {
            let blob: Vec<u8> = row.get(7)?;
            let vector = bytes_to_f32_vec(&blob)?;
            let distance = cosine_distance(query_vector, &vector);

            all_scored.push(VectorMatch {
                chunk_id: row.get(0)?,
                content_block_id: row.get(1)?,
                message_id: row.get(2)?,
                ic: row.get(3)?,
                chunk_index: row.get::<_, i64>(4)? as usize,
                chunk_hash: row.get(5)?,
                text: row.get(6)?,
                distance,
                similarity: 1.0 / (1.0 + distance),
            });
        }

        // Sort by ascending distance (descending similarity)
        all_scored.sort_by(|a, b| a.distance.partial_cmp(&b.distance).unwrap());
        all_scored.truncate(k);
        Ok(all_scored)
    }

    fn count(&self) -> Result<usize> {
        let count: i64 =
            self.conn
                .query_row("SELECT COUNT(*) FROM semantic_blob_chunks", [], |r| {
                    r.get(0)
                })?;
        Ok(count as usize)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sqlite_vec_and_blob_equivalence() {
        ensure_sqlite_vec_registered();
        let conn = Connection::open_in_memory().unwrap();
        let vec_store = SqliteVecStore::new(&conn, 4);
        let blob_store = SqliteBlobStore::new(&conn, 4);

        vec_store.create_tables().unwrap();
        blob_store.create_tables().unwrap();

        let chunk1 = TextChunk {
            content_block_id: "cb-uuid-1".to_string(),
            message_id: "m1".into(),
            ic: Some(10),
            chunk_index: 0,
            total_chunks: 1,
            chunk_hash: "h1".into(),
            text: "premier chunk".into(),
        };

        let chunk2 = TextChunk {
            content_block_id: "cb-uuid-2".to_string(),
            message_id: "m2".into(),
            ic: Some(20),
            chunk_index: 0,
            total_chunks: 1,
            chunk_hash: "h2".into(),
            text: "deuxieme chunk".into(),
        };

        let v1 = [1.0f32, 0.0, 0.0, 0.0];
        let v2 = [0.0f32, 1.0, 0.0, 0.0];

        vec_store.insert_chunk(&chunk1, &v1).unwrap();
        vec_store.insert_chunk(&chunk2, &v2).unwrap();

        blob_store.insert_chunk(&chunk1, &v1).unwrap();
        blob_store.insert_chunk(&chunk2, &v2).unwrap();

        assert_eq!(vec_store.count().unwrap(), 2);
        assert_eq!(blob_store.count().unwrap(), 2);

        let query = [0.9f32, 0.1, 0.0, 0.0];
        let res_vec = vec_store.search_knn(&query, 1).unwrap();
        let res_blob = blob_store.search_knn(&query, 1).unwrap();

        assert_eq!(res_vec.len(), 1);
        assert_eq!(res_blob.len(), 1);
        assert_eq!(res_vec[0].content_block_id, "cb-uuid-1");
        assert_eq!(res_blob[0].content_block_id, "cb-uuid-1");
        assert_eq!(res_vec[0].ic, Some(10));
        assert_eq!(res_blob[0].ic, Some(10));
    }

    #[test]
    fn test_sync_block_chunks_idempotence_and_replacement() {
        ensure_sqlite_vec_registered();
        let conn = Connection::open_in_memory().unwrap();
        let vec_store = SqliteVecStore::new(&conn, 4);
        let blob_store = SqliteBlobStore::new(&conn, 4);

        vec_store.create_tables().unwrap();
        blob_store.create_tables().unwrap();

        let block_id = "cb-test-sync";
        let chunk_v1 = TextChunk {
            content_block_id: block_id.to_string(),
            message_id: "msg-1".into(),
            ic: Some(100),
            chunk_index: 0,
            total_chunks: 1,
            chunk_hash: "hash_v1".into(),
            text: "version une du texte".into(),
        };
        let vec_v1 = vec![1.0f32, 0.0, 0.0, 0.0];

        // 1. Initial insert via sync_block_chunks
        let status_vec1 = vec_store
            .sync_block_chunks(
                block_id,
                std::slice::from_ref(&chunk_v1),
                std::slice::from_ref(&vec_v1),
            )
            .unwrap();
        let status_blob1 = blob_store
            .sync_block_chunks(
                block_id,
                std::slice::from_ref(&chunk_v1),
                std::slice::from_ref(&vec_v1),
            )
            .unwrap();
        assert_eq!(status_vec1, SyncStatus::Inserted { count: 1 });
        assert_eq!(status_blob1, SyncStatus::Inserted { count: 1 });
        assert_eq!(vec_store.count().unwrap(), 1);
        assert_eq!(blob_store.count().unwrap(), 1);

        // 2. Second sync with identical content -> must be UpToDate
        let status_vec2 = vec_store
            .sync_block_chunks(
                block_id,
                std::slice::from_ref(&chunk_v1),
                std::slice::from_ref(&vec_v1),
            )
            .unwrap();
        let status_blob2 = blob_store
            .sync_block_chunks(block_id, &[chunk_v1], &[vec_v1])
            .unwrap();
        assert_eq!(status_vec2, SyncStatus::UpToDate);
        assert_eq!(status_blob2, SyncStatus::UpToDate);
        assert_eq!(vec_store.count().unwrap(), 1);
        assert_eq!(blob_store.count().unwrap(), 1);

        // 3. Third sync with updated content (2 chunks now) -> must be Replaced { old_count: 1, new_count: 2 }
        let chunk_v2_a = TextChunk {
            content_block_id: block_id.to_string(),
            message_id: "msg-1".into(),
            ic: Some(100),
            chunk_index: 0,
            total_chunks: 2,
            chunk_hash: "hash_v2_part1".into(),
            text: "version deux partie un".into(),
        };
        let chunk_v2_b = TextChunk {
            content_block_id: block_id.to_string(),
            message_id: "msg-1".into(),
            ic: Some(100),
            chunk_index: 1,
            total_chunks: 2,
            chunk_hash: "hash_v2_part2".into(),
            text: "version deux partie deux".into(),
        };
        let vec_v2_a = vec![0.0f32, 1.0, 0.0, 0.0];
        let vec_v2_b = vec![0.0f32, 0.0, 1.0, 0.0];

        let status_vec3 = vec_store
            .sync_block_chunks(
                block_id,
                &[chunk_v2_a.clone(), chunk_v2_b.clone()],
                &[vec_v2_a.clone(), vec_v2_b.clone()],
            )
            .unwrap();
        let status_blob3 = blob_store
            .sync_block_chunks(block_id, &[chunk_v2_a, chunk_v2_b], &[vec_v2_a, vec_v2_b])
            .unwrap();

        assert_eq!(
            status_vec3,
            SyncStatus::Replaced {
                old_count: 1,
                new_count: 2
            }
        );
        assert_eq!(
            status_blob3,
            SyncStatus::Replaced {
                old_count: 1,
                new_count: 2
            }
        );
        assert_eq!(vec_store.count().unwrap(), 2);
        assert_eq!(blob_store.count().unwrap(), 2);

        // Verify KNN search reaches newly replaced chunks
        let query = [0.0f32, 0.95, 0.0, 0.0];
        let res = vec_store.search_knn(&query, 1).unwrap();
        assert_eq!(res.len(), 1);
        assert_eq!(res[0].chunk_hash, "hash_v2_part1");
    }
}
