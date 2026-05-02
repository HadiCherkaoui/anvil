-- M2 schema overhaul.
--
-- M1 had no production data, so this is a destructive replacement
-- rather than an ALTER chain. Column names, types, and PK strategy
-- all change: integer unix timestamps, mebibyte memory, exposure_mode
-- replaces service_type, server identity is a UUID.

DROP TABLE IF EXISTS audit_log;
DROP TABLE IF EXISTS servers;

CREATE TABLE servers (
  id              TEXT PRIMARY KEY,                          -- UUID v4
  name            TEXT UNIQUE NOT NULL,                      -- DNS-1123 label
  mc_version      TEXT NOT NULL,
  memory_mi       INTEGER NOT NULL,                          -- mebibytes
  server_type     TEXT NOT NULL,                             -- "vanilla" in M2
  exposure_mode   TEXT NOT NULL,                             -- loadbalancer | nodeport | clusterip
  storage_class   TEXT,                                      -- NULL => use chart default
  storage_size_gi INTEGER NOT NULL DEFAULT 10,
  source_config   TEXT NOT NULL,                             -- JSON, room for future provider configs
  nodeport        INTEGER,                                   -- assigned in 30000..=30099 if exposure_mode=nodeport
  created_at      INTEGER NOT NULL,                          -- unix seconds
  last_started_at INTEGER                                    -- unix seconds, nullable
);

CREATE TABLE audit_log (
  id        INTEGER PRIMARY KEY AUTOINCREMENT,
  ts        INTEGER NOT NULL,                                -- unix seconds
  server_id TEXT NOT NULL,                                   -- not a FK -- survives deletion
  action    TEXT NOT NULL,                                   -- created|started|stopped|restarted|deleted
  details   TEXT,                                            -- nullable JSON blob
  actor     TEXT                                             -- NULL until M4
);

CREATE INDEX idx_audit_server_ts ON audit_log (server_id, ts DESC);
CREATE INDEX idx_servers_nodeport ON servers (nodeport) WHERE nodeport IS NOT NULL;
