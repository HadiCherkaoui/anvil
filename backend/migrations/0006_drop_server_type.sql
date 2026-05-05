-- Drop the legacy `server_type` column. M5 added `source_kind` as the
-- canonical discriminator (`vanilla` | `curseforge` | `modrinth` | `modded` |
-- `paper`). The two columns have been written in lockstep ever since;
-- only `source_kind` is read by the application code now. Requires SQLite ≥ 3.35.

ALTER TABLE servers DROP COLUMN server_type;
