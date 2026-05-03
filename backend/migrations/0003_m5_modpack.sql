-- M5 schema additions for modpack support.
--
-- - `source_kind` distinguishes vanilla rows from CurseForge ones; the
--   existing `source_config` JSON column carries provider-specific config
--   (project_id, channel, version_skip, force_version, current_version_*,
--   auto_update_mode).
-- - `modpack_versions` caches the latest upstream version per server, written
--   by the hourly poller and read by the list/detail handlers.
-- - `idx_audit_server_action` accelerates the audit-by-action lookups the
--   update orchestrator emits.

ALTER TABLE servers ADD COLUMN source_kind TEXT NOT NULL DEFAULT 'vanilla';

CREATE TABLE modpack_versions (
  server_id           TEXT PRIMARY KEY REFERENCES servers(id) ON DELETE CASCADE,
  latest_id           INTEGER NOT NULL,
  latest_name         TEXT NOT NULL,
  latest_download_url TEXT NOT NULL,
  changelog_excerpt   TEXT,
  checked_at          INTEGER NOT NULL
);

CREATE INDEX idx_audit_server_action ON audit_log (server_id, action, ts DESC);
