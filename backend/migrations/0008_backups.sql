CREATE TABLE IF NOT EXISTS backups (
    id              TEXT PRIMARY KEY,
    server_id       TEXT NOT NULL,
    name            TEXT,
    created_at      INTEGER NOT NULL,
    snapshot_path   TEXT NOT NULL,
    mc_version      TEXT NOT NULL,
    memory_mi       INTEGER NOT NULL,
    storage_size_gi INTEGER NOT NULL,
    storage_class   TEXT,
    exposure_mode   TEXT NOT NULL,
    source_kind     TEXT NOT NULL,
    source_config   TEXT NOT NULL,
    size_bytes      INTEGER,
    FOREIGN KEY (server_id) REFERENCES servers(id) ON DELETE CASCADE
);
CREATE INDEX IF NOT EXISTS idx_backups_server ON backups(server_id, created_at DESC);
