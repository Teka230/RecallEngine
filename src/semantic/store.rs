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

/// A matched chunk from vector retrieval.
#[derive(Debug, Clone, PartialEq)]
pub struct VectorMatch {
    pub chunk_id: i64,
    pub content_block_id: i64,
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
                content_block_id INTEGER NOT NULL,
                message_id TEXT NOT NULL,
                ic INTEGER,
                chunk_index INTEGER NOT NULL,
                total_chunks INTEGER NOT NULL,
                chunk_hash TEXT NOT NULL,
                text TEXT NOT NULL
            );

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
                chunk.content_block_id,
                chunk.message_id,
                chunk.ic,
                chunk.chunk_index as i64,
                chunk.total_chunks as i64,
                chunk.chunk_hash,
                chunk.text,
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
                content_block_id INTEGER NOT NULL,
                message_id TEXT NOT NULL,
                ic INTEGER,
                chunk_index INTEGER NOT NULL,
                total_chunks INTEGER NOT NULL,
                chunk_hash TEXT NOT NULL,
                text TEXT NOT NULL,
                embedding BLOB NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_blob_content_block ON semantic_blob_chunks(content_block_id);
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
                chunk.content_block_id,
                chunk.message_id,
                chunk.ic,
                chunk.chunk_index as i64,
                chunk.total_chunks as i64,
                chunk.chunk_hash,
                chunk.text,
                vector_bytes,
            ],
        )?;

        Ok(self.conn.last_insert_rowid())
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
            content_block_id: 1,
            message_id: "m1".into(),
            ic: Some(10),
            chunk_index: 0,
            total_chunks: 1,
            chunk_hash: "h1".into(),
            text: "premier chunk".into(),
        };

        let chunk2 = TextChunk {
            content_block_id: 2,
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
        assert_eq!(res_vec[0].content_block_id, 1);
        assert_eq!(res_blob[0].content_block_id, 1);
        assert_eq!(res_vec[0].ic, Some(10));
        assert_eq!(res_blob[0].ic, Some(10));
    }
}
