-- Per-mod / per-plugin update tracking. The hourly poller compares each
-- installed mod (modded servers) and plugin (paper servers) against the
-- latest compatible upstream version and writes a row here when a newer
-- version is available. The same table covers both mods and plugins —
-- the `provider` column distinguishes Modrinth vs CurseForge; nothing in
-- the schema is mod- or plugin-specific.
--
-- Rows are deleted when the installed version catches up with upstream
-- so the frontend can simply count rows to show the "X updates available"
-- banner.

CREATE TABLE IF NOT EXISTS mod_updates (
    server_id              TEXT NOT NULL,
    provider               TEXT NOT NULL,             -- 'modrinth' | 'curseforge'
    project_id             TEXT NOT NULL,
    current_version_id     TEXT NOT NULL,
    latest_version_id      TEXT NOT NULL,
    latest_version_name    TEXT NOT NULL,
    latest_published_at    TEXT,                      -- ISO 8601 string from upstream
    checked_at             INTEGER NOT NULL,          -- unix seconds
    PRIMARY KEY (server_id, provider, project_id),
    FOREIGN KEY (server_id) REFERENCES servers(id) ON DELETE CASCADE
);

CREATE INDEX IF NOT EXISTS idx_mod_updates_server ON mod_updates(server_id);
