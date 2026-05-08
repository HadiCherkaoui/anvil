-- Drop the unused `changelog_excerpt` column. Added in 0003 with the
-- intent of surfacing per-version changelogs in the update banner, but no
-- code path ever populated it: the poller only writes (id, name,
-- download_url, checked_at), and no SELECT reads the column. Removing it
-- so the schema reflects reality. Requires SQLite >= 3.35.

ALTER TABLE modpack_versions DROP COLUMN changelog_excerpt;
