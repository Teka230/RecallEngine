-- PR 1: FTS5 Search Index for Conversation Titles
CREATE VIRTUAL TABLE IF NOT EXISTS conversations_fts USING fts5(
  title,
  content='conversations',
  content_rowid='rowid',
  tokenize='unicode61 remove_diacritics 2'
);

CREATE TRIGGER IF NOT EXISTS conversations_ai AFTER INSERT ON conversations BEGIN
  INSERT INTO conversations_fts(rowid, title) VALUES (new.rowid, new.title);
END;

CREATE TRIGGER IF NOT EXISTS conversations_ad AFTER DELETE ON conversations BEGIN
  INSERT INTO conversations_fts(conversations_fts, rowid, title) VALUES ('delete', old.rowid, old.title);
END;

CREATE TRIGGER IF NOT EXISTS conversations_au AFTER UPDATE ON conversations BEGIN
  INSERT INTO conversations_fts(conversations_fts, rowid, title) VALUES ('delete', old.rowid, old.title);
  INSERT INTO conversations_fts(rowid, title) VALUES (new.rowid, new.title);
END;
