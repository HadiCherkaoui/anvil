-- M6: per-server CPU limit (millicores). Default 1000 = 1 core matches the
-- v1 hardcoded "2000m" floor only loosely — existing servers keep running
-- with their old StatefulSet spec until the next restart picks up the new
-- value. The migration backfills 1000 so subsequent reads/writes are valid.

ALTER TABLE servers ADD COLUMN cpu_millicores INTEGER NOT NULL DEFAULT 1000;
