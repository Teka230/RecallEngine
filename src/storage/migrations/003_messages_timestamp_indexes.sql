-- Indexes for day-range and role-filtered reads (ExploGPT Journal / activity APIs).
CREATE INDEX IF NOT EXISTS idx_messages_timestamp ON messages(timestamp);
CREATE INDEX IF NOT EXISTS idx_messages_timestamp_role ON messages(timestamp, role);
CREATE INDEX IF NOT EXISTS idx_messages_model_slug ON messages(model_slug);
