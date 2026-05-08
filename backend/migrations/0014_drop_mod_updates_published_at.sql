-- Drop the unused `latest_published_at` column. Added in 0007 to feed a
-- "newer than X" hint in the per-mod update UI; the UI never landed and
-- no SELECT reads the column. The poller still binds it on upsert via a
-- struct field that exists only to satisfy this dead column. Drop both.
-- Requires SQLite >= 3.35.

ALTER TABLE mod_updates DROP COLUMN latest_published_at;
