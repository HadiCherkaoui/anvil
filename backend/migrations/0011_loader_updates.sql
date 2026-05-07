-- Surfaces newer loader versions for modded forge/neoforge servers.
-- Fabric uses itzg's LATEST behaviour and never pins, so it doesn't need
-- this table. Paper is also out of scope here — paper_build is rarely
-- pinned in the homelab profile.
--
-- One row per server when an update is available; row is deleted when
-- the loader matches the latest published version. Cascade with the
-- server row so a deletion cleans up the entry.
CREATE TABLE IF NOT EXISTS loader_updates (
    server_id      TEXT PRIMARY KEY,
    current_loader TEXT NOT NULL,
    latest_loader  TEXT NOT NULL,
    checked_at     INTEGER NOT NULL,
    FOREIGN KEY (server_id) REFERENCES servers(id) ON DELETE CASCADE
);
