-- Migration 004: Semantic Search Vector Tables & Index Metadata
-- Tables for deterministic chunks, sqlite-vec virtual items, and index lifecycle metadata.

CREATE TABLE IF NOT EXISTS semantic_vec_chunks (
  chunk_id           INTEGER PRIMARY KEY AUTOINCREMENT,
  content_block_id   TEXT NOT NULL REFERENCES content_blocks(id) ON DELETE CASCADE,
  message_id         TEXT NOT NULL REFERENCES messages(id) ON DELETE CASCADE,
  ic                 INTEGER,
  chunk_index        INTEGER NOT NULL,
  total_chunks       INTEGER NOT NULL,
  chunker_version    TEXT NOT NULL DEFAULT 'v1-512-64',
  model_id           TEXT NOT NULL DEFAULT 'intfloat/multilingual-e5-small',
  chunk_hash         TEXT NOT NULL,
  fingerprint        TEXT NOT NULL,
  text               TEXT NOT NULL,
  created_at         TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now')),
  UNIQUE (content_block_id, chunk_index)
);

CREATE INDEX IF NOT EXISTS idx_vec_chunks_block ON semantic_vec_chunks(content_block_id);
CREATE INDEX IF NOT EXISTS idx_vec_chunks_hash ON semantic_vec_chunks(chunk_hash);
CREATE INDEX IF NOT EXISTS idx_vec_chunks_fingerprint ON semantic_vec_chunks(fingerprint);
CREATE INDEX IF NOT EXISTS idx_vec_chunks_message ON semantic_vec_chunks(message_id);
CREATE INDEX IF NOT EXISTS idx_vec_chunks_ic ON semantic_vec_chunks(ic);

CREATE VIRTUAL TABLE IF NOT EXISTS semantic_vec_items USING vec0(
  chunk_id INTEGER PRIMARY KEY,
  embedding float[384] distance_metric=cosine
);

CREATE TABLE IF NOT EXISTS semantic_index_meta (
  key        TEXT PRIMARY KEY,
  value      TEXT NOT NULL,
  updated_at TEXT NOT NULL DEFAULT (strftime('%Y-%m-%dT%H:%M:%fZ', 'now'))
);

CREATE TRIGGER IF NOT EXISTS trg_semantic_vec_chunks_delete
AFTER DELETE ON semantic_vec_chunks
BEGIN
  DELETE FROM semantic_vec_items WHERE chunk_id = OLD.chunk_id;
END;

