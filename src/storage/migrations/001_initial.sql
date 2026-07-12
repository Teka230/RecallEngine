-- RecallEngine canonical schema v1

CREATE TABLE IF NOT EXISTS import_runs (
  id                TEXT PRIMARY KEY,
  source_root       TEXT NOT NULL,
  started_at        TEXT NOT NULL,
  completed_at      TEXT,
  status            TEXT NOT NULL
    CHECK (status IN ('running','completed','failed','partial')),
  strict_mode       INTEGER NOT NULL DEFAULT 0,
  stats_json        TEXT,
  error_summary     TEXT
);

CREATE TABLE IF NOT EXISTS source_files (
  id                INTEGER PRIMARY KEY AUTOINCREMENT,
  import_run_id     TEXT NOT NULL REFERENCES import_runs(id),
  relative_path     TEXT NOT NULL,
  kind              TEXT NOT NULL,
  size_bytes        INTEGER,
  sha256            TEXT NOT NULL,
  status            TEXT NOT NULL
    CHECK (status IN ('seen','imported','skipped','failed')),
  UNIQUE (import_run_id, relative_path)
);
CREATE INDEX IF NOT EXISTS idx_source_files_path_hash ON source_files(relative_path, sha256);

CREATE TABLE IF NOT EXISTS conversations (
  id                    TEXT PRIMARY KEY,
  title                 TEXT,
  create_time           REAL,
  update_time           REAL,
  current_node_id       TEXT,
  default_model_slug    TEXT,
  is_archived           INTEGER NOT NULL DEFAULT 0,
  is_starred            INTEGER NOT NULL DEFAULT 0,
  source_relative_path  TEXT NOT NULL,
  last_seen_import_run_id TEXT NOT NULL REFERENCES import_runs(id),
  is_active             INTEGER NOT NULL DEFAULT 1,
  raw_json              TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS nodes (
  id                    TEXT PRIMARY KEY,
  conversation_id       TEXT NOT NULL REFERENCES conversations(id),
  parent_id             TEXT,
  has_message           INTEGER NOT NULL DEFAULT 0,
  source_relative_path  TEXT NOT NULL,
  last_seen_import_run_id TEXT NOT NULL REFERENCES import_runs(id),
  is_active             INTEGER NOT NULL DEFAULT 1,
  raw_json              TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_nodes_conversation ON nodes(conversation_id);
CREATE INDEX IF NOT EXISTS idx_nodes_parent ON nodes(parent_id);

CREATE TABLE IF NOT EXISTS messages (
  id                    TEXT PRIMARY KEY,
  ic                    INTEGER NOT NULL UNIQUE,
  node_id               TEXT NOT NULL REFERENCES nodes(id),
  conversation_id       TEXT NOT NULL REFERENCES conversations(id),
  role                  TEXT,
  author_name           TEXT,
  create_time           REAL,
  create_time_raw       REAL,
  timestamp             TEXT,
  source_shard_index    INTEGER NOT NULL,
  source_node_order     INTEGER NOT NULL,
  model_slug            TEXT,
  content_type          TEXT,
  source_relative_path  TEXT NOT NULL,
  last_seen_import_run_id TEXT NOT NULL REFERENCES import_runs(id),
  is_active             INTEGER NOT NULL DEFAULT 1,
  raw_json              TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_messages_conversation ON messages(conversation_id);
CREATE INDEX IF NOT EXISTS idx_messages_node ON messages(node_id);
CREATE INDEX IF NOT EXISTS idx_messages_ic ON messages(ic);

CREATE TABLE IF NOT EXISTS content_blocks (
  id                TEXT PRIMARY KEY,
  message_id        TEXT NOT NULL REFERENCES messages(id),
  ordinal           INTEGER NOT NULL,
  kind              TEXT NOT NULL,
  text_content      TEXT,
  json_content      TEXT,
  UNIQUE (message_id, ordinal)
);

CREATE TABLE IF NOT EXISTS assets (
  id                TEXT PRIMARY KEY,
  source_key        TEXT NOT NULL UNIQUE,
  display_name      TEXT,
  source_filename   TEXT,
  relative_path     TEXT,
  mime_type         TEXT,
  size_bytes        INTEGER,
  sha256            TEXT,
  exists_locally    INTEGER NOT NULL DEFAULT 0,
  last_seen_import_run_id TEXT REFERENCES import_runs(id),
  is_active         INTEGER NOT NULL DEFAULT 1,
  raw_json          TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS message_assets (
  message_id        TEXT NOT NULL REFERENCES messages(id),
  asset_id          TEXT NOT NULL REFERENCES assets(id),
  link_source       TEXT NOT NULL,
  ordinal           INTEGER NOT NULL DEFAULT 0,
  raw_json          TEXT NOT NULL,
  PRIMARY KEY (message_id, asset_id, link_source, ordinal)
);

CREATE TABLE IF NOT EXISTS content_references (
  id                TEXT PRIMARY KEY,
  message_id        TEXT NOT NULL REFERENCES messages(id),
  ordinal           INTEGER NOT NULL,
  ref_source        TEXT NOT NULL,
  raw_json          TEXT NOT NULL,
  UNIQUE (message_id, ordinal, ref_source)
);

CREATE TABLE IF NOT EXISTS feedback (
  id                TEXT PRIMARY KEY,
  message_id        TEXT,
  rating            TEXT,
  tags              TEXT,
  text              TEXT,
  created_at        TEXT,
  source_relative_path TEXT NOT NULL DEFAULT 'message_feedback.json',
  last_seen_import_run_id TEXT NOT NULL REFERENCES import_runs(id),
  is_active         INTEGER NOT NULL DEFAULT 1,
  raw_json          TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS shared_conversations (
  id                TEXT PRIMARY KEY,
  conversation_id   TEXT,
  share_id          TEXT,
  url               TEXT,
  created_at        TEXT,
  is_anonymous      INTEGER NOT NULL DEFAULT 0,
  source_relative_path TEXT NOT NULL DEFAULT 'shared_conversations.json',
  last_seen_import_run_id TEXT NOT NULL REFERENCES import_runs(id),
  is_active         INTEGER NOT NULL DEFAULT 1,
  raw_json          TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS library_files (
  id                TEXT PRIMARY KEY,
  file_id           TEXT,
  file_name         TEXT,
  mime_type         TEXT,
  file_size_bytes   INTEGER,
  sha256_digest     TEXT,
  source_relative_path TEXT NOT NULL DEFAULT 'library_files.json',
  last_seen_import_run_id TEXT NOT NULL REFERENCES import_runs(id),
  is_active         INTEGER NOT NULL DEFAULT 1,
  raw_json          TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS import_issues (
  id                INTEGER PRIMARY KEY AUTOINCREMENT,
  import_run_id     TEXT NOT NULL REFERENCES import_runs(id),
  severity          TEXT NOT NULL
    CHECK (severity IN ('error','warning','info')),
  code              TEXT NOT NULL,
  entity_type       TEXT,
  entity_id         TEXT,
  source_relative_path TEXT,
  message           TEXT NOT NULL,
  created_at        TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_import_issues_run ON import_issues(import_run_id);

-- FTS5 Search Index
CREATE VIRTUAL TABLE IF NOT EXISTS content_blocks_fts USING fts5(
  text_content,
  content='content_blocks',
  content_rowid='rowid',
  tokenize='unicode61 remove_diacritics 2'
);

CREATE TRIGGER IF NOT EXISTS content_blocks_ai AFTER INSERT ON content_blocks BEGIN
  INSERT INTO content_blocks_fts(rowid, text_content)
  VALUES (new.rowid, new.text_content);
END;

CREATE TRIGGER IF NOT EXISTS content_blocks_ad AFTER DELETE ON content_blocks BEGIN
  INSERT INTO content_blocks_fts(content_blocks_fts, rowid, text_content)
  VALUES('delete', old.rowid, old.text_content);
END;

CREATE TRIGGER IF NOT EXISTS content_blocks_au AFTER UPDATE ON content_blocks BEGIN
  INSERT INTO content_blocks_fts(content_blocks_fts, rowid, text_content)
  VALUES('delete', old.rowid, old.text_content);
  INSERT INTO content_blocks_fts(rowid, text_content)
  VALUES (new.rowid, new.text_content);
END;
