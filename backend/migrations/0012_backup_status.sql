-- Tracks the lifecycle of a backup row so a crashed orchestrator does
-- not leave the UI advertising a tarball that never finished writing.
-- Inserted as 'pending' before the tar Job runs; set to 'complete' after
-- it succeeds, or 'failed' on Job failure / row-stale GC.
-- Existing rows backfill as 'complete' via DEFAULT.
ALTER TABLE backups ADD COLUMN status TEXT NOT NULL DEFAULT 'complete';
