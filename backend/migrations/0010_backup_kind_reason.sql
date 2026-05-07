-- Distinguishes user-triggered ("manual") from FSM-triggered ("auto")
-- backup rows so the Backups tab can surface both. `reason` is a
-- free-form context string ("modpack-update:v2.3.1",
-- "mc-version-change:1.20.4->1.21") populated by the auto orchestrators;
-- NULL for manual rows. Existing rows backfill as 'manual' via DEFAULT.
ALTER TABLE backups ADD COLUMN kind   TEXT NOT NULL DEFAULT 'manual';
ALTER TABLE backups ADD COLUMN reason TEXT;

-- The list endpoint paginates by (server_id, created_at DESC) regardless
-- of kind, so the existing index still covers it. No new index needed.
